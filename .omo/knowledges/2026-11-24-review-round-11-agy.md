# 第十一轮代码审查报告（`origin/master` `e80ce17`）

## 审查基线与覆盖边界

- **审查基线**：`origin/master` `e80ce17783810686c22ee173fb15af796c3a56f6`（`e80ce17`）。
- **第十轮修复范围**：`c4f72a2..e80ce17`（共 18 个提交，逐条对应 `.omo/knowledges/2026-11-24-review-round-10-fixes.md` 中的 F01–F17 与共享 ACL 重构 `900737d`）。
- **只读约束**：本轮未修改任何源码文件，未执行 `git commit` 或 `git push`。所有独立探针脚本、测试用构造夹具（仅用于复现验证，非生产代码）与运行日志保存在 `.omo/reviews/2026-11-25/agy/`。
- **平台验证边界**：
  - **Linux x86_64**：实测执行 `cargo fmt --check`、`xtask` 策略检查与单测、核心 crate 单元/集成测试、前端单测/类型/Biome 检查、`scripts/test-runtime-references.sh`、`scripts/test-e2e-output-reuse.sh`、自建探针以及真实构建二进制上的 `scripts/smoke-release.sh`。
  - **Windows**：通过 `CFLAGS_x86_64_pc_windows_gnu=-D_WIN32_WINNT=0x0601 cargo clippy --locked --target x86_64-pc-windows-gnu --workspace --all-targets --all-features -- -D warnings` 完成编译期校验；运行时行为基于仓库源码、`interprocess 2.4.3` 源码与 Win32 API 规范静态核验，并在每条结论中明确标注“实测复现”或“静态核验”。
  - **macOS**：基于源码、依赖库源码及 POSIX/APFS/Bash 3.2 规范静态核验。

### 核心结论摘要

第十轮修复关闭了 F02、F04、F06、F10（别名解析本身）、F12、F14，但 **F01、F03、F05、F07、F08、F09、F11、F13、F15、F16、F17 均存在未闭环缺口或同模块漏网缺陷**。全仓审查另定位到多处高危阻断与安全问题，优先级最高的是：

1. **发版烟雾测试全平台必挂（F01，实测复现）**：第十轮修通 JS 扫描后，`smoke-release.sh` 推进到 CSS 引用的自托管 `.woff2` 字体，因脚本未列 `*.woff2` 媒体类型而无条件失败（`font/woff2` != `application/octet-stream`）。
2. **Windows 本地 IPC 与 Process 监管通道设计性失败（F02，静态核验）**：服务端在未从命名管道读取任何字节前调用 `ImpersonateNamedPipeClient`，按 Windows `npfs.sys` 规范返回 `ERROR_CANNOT_IMPERSONATE`；且 `OpenThreadToken` 使用 `OpenAsSelf = FALSE`。CI 在 Windows 上从未运行 IPC/supervisor 测试，未能暴露。
3. **Windows 两处私密 ACL 校验绕过（F03，部分实测复现）**：`sddl::is_private` 接受受保护 NULL DACL `D:PNO_ACCESS_CONTROL`（实测返回 `true`）；`ma2a-store` 的 `validate_windows_acl` 过滤 `!$_.IsInherited`，静默放行受保护 DACL 中带 `INHERITED_ACE` 标记的宽松规则。
4. **未认证远端 DoS 与核心 Actor 饥饿（F05、F06、F07）**：未认证远端节点可永久耗尽 Control Sync 全部 16 个全局槽位、借 `EnrollmentHandler` + `biased select!` 饥饿整个 `Actor::run_loop`，并耗尽全部 8 类流式服务的全局并发配额。
5. **私密中继撤销绕过、多 Space 撤销断链与守护进程崩溃/挂死（F08、F10、F11、F12、F18）**：`PrivateRelayAccess` 握手期撤销存在 TOCTOU 永久残留；单 Space 撤销永久阻断其他共享 Space 的出站控制同步；`remove_tree` 无界递归可致栈溢出崩溃或 FD 耗尽；`ConnectionManager` 128 连接槽位泄漏；守护进程停机超时在 `#[tokio::main]` `Runtime::drop` 处挂死且继续占用 `SINGLETON_LOCK`。

---

## 第一部分：第十轮 F01–F17 与 `900737d` 逐项复核

| 第十轮项 | 修复提交 | 核验状态 | 覆盖平台核验（Linux / macOS / Windows） | 核验结论与遗留/回归摘要 |
| --- | --- | --- | --- | --- |
| **F01** | `a18c021` | **部分关闭** | 全平台均有遗留 | 当前产物 `web/dist/assets/index-BrFGfxRh.js` 可通过扫描，但 `extract-runtime-references.pl` 对模板 `${...}`、正则/除法歧义、`window.fetch`、转义斜杠等存在可复现漏报与误报（见 **F28**）；同时 F01 通关后立即触发 `smoke-release.sh` 拒绝 `.woff2`（见 **F01**）以及 macOS 默认 Bash 3.2 `set -u` 数组空切片崩溃（见 **F28**）。 |
| **F02** | `4f263d8` | **已关闭** | Linux 实测；macOS/Windows 静态核验 | 暂存名加入 16 字节随机 nonce（`.ma2a-upload-<req><nonce>.part`），创建失败立即 `journal.forget`，`is_runtime_reserved` 与 `is_staging_file` 对齐。关联新缺口：`remove_tree` 仍会删除活跃 `.ma2a-upload-*`（见 **F25**），取消上传时丢弃 future 会跳过 `journal.forget` 残留记录（见 **F21**）。 |
| **F03** | `50bf871` | **部分关闭** | Windows 遗留 | `ipc/windows.rs` 在逐级创建缺失目录时，若中间层级遇 `AlreadyExists` 仅置 `created_leaf = false` 并继续创建叶目录，后续只校验叶目录而放过被竞态抢占的中间目录；且 `ma2a-store/src/permissions.rs`（`create_private_dir`、`create_new_private_dir`）未改造，影响 `a2a/token_file.rs`、`backup.rs` 与直接 `Repository::open`/`KeyStore::open`（见 **F04**）。 |
| **F04** | `b80633a` | **已关闭** | Linux 实测；macOS/Windows 静态核验 | `existing()` 改为先打开常规文件再核对规范路径与文件身份，并复用包含上标 `¹²³` 的 `is_windows_reserved`。仅余一次要次序缺陷：`agents/tools/files.rs:141-161` 在校验 `partial.directory_is_at(parent)` 前即调用 `Partial::open(..., false)`，会提前创建 `.ma2a-downloads` 并截断同名旧 partial（在已文档化的同用户边界内）。 |
| **F05** | `0270965` | **部分关闭** | 全平台均有遗留 | 撤销和 15 秒 `FILES_DEADLINE` 已能通过 `Stop` 终止递归删除；但 `files.rs:42-64` 不监听客户端 `FrameKind::Cancel`/断流，`ops::list`、`ops::mkdir(parents=true)`、`files.rs:171` 写提交与 `transfer/upload.rs:137` 最终提交均未接入 `Stop`（见 **F21**）。 |
| **F06** | `8126a21` | **已关闭** | Linux 实测；macOS/Windows 静态核验 | `download_content` 按 `identity.size() - offset` 逐帧拦截超长数据并校验 `result.bytes`/`result.size`/`result.hash`，全部 4 处调用方已同步。关联缺口：`Partial::begin` 在续传身份不匹配时覆写 sidecar，且 `client::transfer::upload` 未校验 `result.bytes == size`（见 **F29**）。 |
| **F07** | `2fe2089` | **部分关闭** | 全平台均有遗留 | `authenticated_mutation` 已将 `AuthFailure::Internal` 映射为 `500`，`logoutSession` 增加了单次 CSRF 重试；但失效会话与 CSRF/Origin 拒绝仍同为 `403`，`app.tsx:116-122` 仍吞掉一切 `403` 显示已登出；读接口/SSE/SPA 仍把 `AuthFailure::Internal` 映成 `401`；且多标签登录 CSRF 轮换仅修 `logout`，旧标签其余所有变更均永久 403（见 **F19**）。 |
| **F08** | `6d6f298` | **部分关闭** | 全平台均有大面积遗留 | 仅覆盖 `exec.rs`、`shell.rs`、`reply.rs`、`processes/{attach,stdin,host}.rs`。`stream_host/inbound.rs`、`proxy.rs`、`transfer/{upload,download}.rs`、`processes/{start,reads}.rs`、`agents/{host,watch}.rs`、`stream_plane/relay.rs` 及出站 `ServiceClient::open` 仍无超时或取消边界（见 **F09**）；且 `reply.rs`、`hosting/call.rs` 在 torn 流上仍调用 `shutdown()` 发送 QUIC `FIN`（见 **F23**）。 |
| **F09** | `c30bd79` | **部分关闭** | Windows 遗留 | LocalSystem 1/2 条 ACE 处理正确，`apply_windows_acl` 已去重 SID；但 `ma2a-store/src/permissions.rs:198` 仍使用 `Where-Object {!$_.IsInherited}`，导致受保护 DACL 中带 `INHERITED_ACE` 标记的宽松规则绕过校验（见 **F03**）。 |
| **F10** + `900737d` | `bfc1836`, `900737d` | **已关闭（关联绕过见 F03）** | Windows 静态核验 + Linux 实测 `sddl` | `ConvertStringSidToSidW` 原生解析 `LA` 等 SDDL 别名，共享模块 `windows_acl.rs` / `sddl.rs` 归档与 `xtask` 登记完整；但同文件 `sddl.rs:24-38` 接受受保护 NULL DACL `D:PNO_ACCESS_CONTROL`（见 **F03**）。 |
| **F11** | `a424b93` | **部分关闭** | Windows 命名边界；Linux/macOS 写错残留 | Windows 裸文件名已解析为 `.` 并通过固定目录发布；但 `\\?\` 逐字路径拼接未过滤尾随点/空格、设备名或 `:`，且 Unix 写失败会留下半截目标文件（见 **F29**）。 |
| **F12** | `7b4934b` | **已关闭** | 全平台关闭 | `has_bytes` 返回 `io::Result<bool>`，`settle` 在本地 I/O 失败时保留 partial 并同时报告流错误与本地错误，无回归。 |
| **F13** | `e011159` | **部分关闭** | 全平台均有遗留 | 额外子目录已被拦截，但 `validate-release.sh` 只核对文件名不核对条目类型（实测含 `ma2a -> /bin/sh` 软链的 `.tar.xz` 归档以退出码 0 通过），且 `tr -d '\r'` 抹平文件名中的 CR（见 **F28**）。 |
| **F14** | `4d452a1` | **已关闭（含低危边界）** | 全平台基本关闭 | `documentDescribes` 唯一性与指向 `ma2a-app` 已强制验证。低危遗留：未校验 `SPDXID` 为非 null 字符串，且前端许可证未规范化 `/`、`.licenses.json` 含构建机绝对路径（见 **F31**）。 |
| **F15** | `2e6abb4` | **部分关闭** | 全平台均有遗留 | 已过滤 `:`、`\`、控制字符及超长 UTF-8；但 `files/ops.rs:91-93` 未过滤 `is_runtime_reserved`（`.ma2a-downloads`、`.ma2a-upload-*`、`.ma2a-write-*`）和 Windows 保留名，仍列出后续请求必拒的名称并暴露活跃暂存文件名（见 **F25**）。 |
| **F16** | `c2c4a58` | **部分关闭** | 全平台均有遗留 | `éunsafe` 误报已消除；但未将 Rust 合法 `Pattern_White_Space`（如 `U+200E`/`U+2028`）视作空白，实测可用合法 Rust 源码绕过 `unsafe` 和 `#[allow(unsafe_code)]` 检测（见 **F30**）。 |
| **F17** | `912bdc3` | **部分关闭** | 全平台均有遗留 | 同行 `code /* comment` 已修；但 `track_tail` 未处理 Rust 字符字面量 `'"'`、原始字符串 `r#"..."#`、多行字符串/模板、JS/TS 正则 `/a\/*/` 和属性行 `#[...] /*`，实测可彻底绕过 250 LOC 限制或误报合法短文件（见 **F28**）。 |

---

## 第二部分：本轮新发现与未完全关闭项

### 高严重度（High）

#### F01 · [高] `scripts/smoke-release.sh` 无条件拒绝内置 `.woff2` 字体，阻断全平台发版
- **平台范围**：Linux、macOS、Windows（全部 6 个发版目标）
- **位置**：
  - `scripts/smoke-release.sh:110-120`
  - `crates/ma2a-runtime/src/web/assets.rs:38`
  - `web/dist/index.html:13`
  - `.github/workflows/release.yml:98-100`
- **证据（实测复现）**：
  1. `web/dist/index.html` 引用 `assets/index-DFjOn4CL.css`。实测 `./scripts/extract-runtime-references.pl css web/dist/assets/index-*.css` 输出两条资源：
     - `asset	/assets/space-grotesk-latin-wght-normal-BhU9QXUp.woff2`
     - `asset	/assets/jetbrains-mono-latin-wght-normal-B9CIFXIH.woff2`
  2. `crates/ma2a-runtime/src/web/assets.rs:38` 对 `.woff2` 返回 `Content-Type: font/woff2`（并由 `assets.rs:71-79` 单测固化）。
  3. `scripts/smoke-release.sh:110-116` 的 `case` 仅列举 `*.html`、`*.js`、`*.css`、`*.svg`，其余扩展名一律落入 `*) expected_type='application/octet-stream' ;;`。
  4. 本轮使用实际构建的 `ma2a` 打包测试归档（仅含真实二进制的 `.tar.xz` 测试夹具）直接运行 `./scripts/smoke-release.sh`，实测输出（日志 `.omo/reviews/2026-11-25/agy/probe-smoke-woff2.log`）：
     ```text
     embedded asset /assets/space-grotesk-latin-wght-normal-BhU9QXUp.woff2 returned unexpected content type font/woff2
     SMOKE_EXIT=1
     ```
- **触发条件与影响**：第十轮因 F01 在扫描 `index.js` 时提前退出，从未走到 CSS 引用队列；且 `.github/workflows/ci.yml:76-79` 只运行 `test-runtime-references.sh` 而非 `smoke-release.sh`。如今任何一次干净构建触发 `.github/workflows/release.yml:100` 均会在 `.woff2` 校验处立即失败，阻断所有平台发版。
- **修复与验收建议**：
  - 在 `scripts/smoke-release.sh:110-116` 增加 `*.woff2) expected_type='font/woff2' ;;`（并与 `crates/ma2a-runtime/src/web/assets.rs:33-40` 支持的所有媒体类型保持同步）。
  - 在 CI 中增加至少 Linux 目标的 `smoke-release.sh` 执行（或对真实打包归档运行），验收标准：`smoke-release.sh` 成功拉取并校验 `.html`/`.js`/`.css`/`.svg`/`.woff2` 全部内置资源并以退出码 `0` 通过。

---

#### F02 · [高] Windows 命名管道鉴权在读取任何字节前调用 `ImpersonateNamedPipeClient`（`ERROR_CANNOT_IMPERSONATE`）且 `OpenThreadToken` 未传 `OpenAsSelf = TRUE`
- **平台范围**：Windows
- **位置**：
  - `crates/ma2a-runtime/src/ipc/server/dispatch.rs:31-32`
  - `crates/ma2a-services/src/supervisor/host_serve.rs:46-52`
  - `crates/ma2a-runtime/src/ipc/windows.rs:108-116`
  - `crates/ma2a-services/src/supervisor/address.rs:127-136`
  - `crates/ma2a-services/src/windows_security.rs:155-176`
- **证据（静态核验 + `interprocess 2.4.3` 源码核验）**：
  1. 核验 `Cargo.lock` 锁定的 `interprocess 2.4.3`：`src/os/windows/named_pipe/c_wrappers.rs:158-166` 客户端以 `CreateFileW(..., FILE_FLAG_OVERLAPPED, ...)` 连接（未设 `SECURITY_SQOS_PRESENT`）；`src/os/windows/named_pipe/tokio/listener.rs:48-58` 的 `accept()` 仅完成 `ConnectNamedPipe`，未执行任何 `ReadFile`。
  2. Windows `npfs.sys` 对 `ImpersonateNamedPipeClient` 的契约明确要求：服务端必须在管道实例上完成至少一次 `ReadFile` 后，内核才会将客户端安全上下文绑定到该管道实例；在此之前调用 `ImpersonateNamedPipeClient` 返回 `FALSE` 且错误码为 `ERROR_CANNOT_IMPERSONATE`（1368）。
  3. `ipc/server/dispatch.rs:31` 在第 32 行 `read_frame` 之前调用 `platform::authorize(&stream, &context.paths)?`；`supervisor/host_serve.rs:46` 在第 52 行 `prove_supervisor` 读取首帧之前调用 `super::address::same_user(&stream)`。两者均直接调用 `impersonated_client_user_sid(stream)` -> `ImpersonateNamedPipeClient(pipe)`。
  4. `windows_security.rs:172` 调用 `OpenThreadToken(thread, TOKEN_QUERY, 0, &raw mut token)`，`OpenAsSelf = FALSE (0)` 会用被模拟客户端的上下文检查令牌访问权；当客户端以 `SECURITY_IDENTIFICATION` 或受限令牌连接时必然返回 `ERROR_ACCESS_DENIED`。
  5. `.github/workflows/ci.yml:35-58` 在 Windows 上只跑了 8 组文件/ACL 精选测试，未运行任何本地 IPC 或 supervisor 集成测试，因此未能发现。
- **触发条件与影响**：在 Windows 上启动守护进程后，本地 CLI（`ma2a status`/`stop`/`space` 等）、Web 后端到守护进程连接、A2A 适配器以及 Process/Agent supervisor 通道在首帧前鉴权即被拒绝，核心功能不可用。
- **修复与验收建议**：
  - 将 Windows 命名管道上的 `impersonated_client_user_sid` 延后至完成首次有界读之后执行（或在本地 IPC/supervisor 协议层先读首帧/最小有界前导字节，再在执行任何命令或回应前完成模拟与 SID 比对；或直接采用 `GetNamedPipeClientProcessId` 打开客户端进程令牌核验 SID，同时留意权限边界）。
  - 将 `windows_security.rs:172` 的 `OpenThreadToken` 第三参数改为 `1`（`OpenAsSelf = TRUE`）。
  - 在 `.github/workflows/ci.yml` 的 `runner.os == 'Windows'` 步骤增加本地 IPC 往返测试与 supervisor 握手测试，验收标准：Windows 原生 runner 上 `LocalApiServer` 接受同用户客户端并完成请求/拒绝异用户，且 supervisor 完成双向证明。

---

#### F03 · [高] Windows 两处私密 ACL 校验绕过：`sddl::is_private` 接受 `D:PNO_ACCESS_CONTROL`；`ma2a-store` 忽略受保护 DACL 中的 `IsInherited` 规则
- **平台范围**：Windows（`sddl.rs` 逻辑跨平台可测）
- **位置**：
  - `crates/ma2a-services/src/sddl.rs:24-38`（调用方 `crates/ma2a-services/src/client/resume/anchor_windows.rs:102-106`）
  - `crates/ma2a-store/src/permissions.rs:159-170, 195-201`
- **证据（实测复现 + 静态核验）**：
  1. **`sddl::is_private` 实测复现**：`sddl.rs:24-28` 以首个 `'('` 切分 `(flags, entries)`，仅检查 `flags.contains('P')`，随后对 `entries.split_terminator(')').all(...)` 求全称量词。当安全描述符设置 `SE_DACL_PRESENT | SE_DACL_PROTECTED` 且 DACL 指针为 `NULL`（Windows 内核含义为授予 Everyone 完全控制）时，SDDL 文本为 `D:PNO_ACCESS_CONTROL`（或 `D:PARNO_ACCESS_CONTROL` / `D:PAINO_ACCESS_CONTROL`）。此时 `entries` 为空串，`.all(...)` 恒真。使用独立探针 `.omo/reviews/2026-11-25/agy/probe_sddl.rs` 直接调用源码 `sddl::is_private`，实测输出（`.omo/reviews/2026-11-25/agy/probe-sddl.log`）：
     ```text
     is_private("O:BAD:PNO_ACCESS_CONTROL") = true
     is_private("O:S-1-5-21-1-2-3-1001D:PARNO_ACCESS_CONTROL") = true
     is_private("O:S-1-5-21-1-2-3-1001D:PAINO_ACCESS_CONTROL") = true
     is_private("O:BAD:P(A;;FA;;;SY") = true
     is_private("O:BAD:NO_ACCESS_CONTROL") = false
     ```
     同时可见未闭合 ACE `O:BAD:P(A;;FA;;;SY` 也被误判为 `true`。
  2. **`ma2a-store` `validate_windows_acl` 静态核验**：`permissions.rs:198` 在检查 `$a.AreAccessRulesProtected` 后执行 `$r=@($a.Access|Where-Object {!$_.IsInherited})`，仅校验 `$r`。但在 Windows NT 安全描述符中，`SE_DACL_PROTECTED` 仅阻止后续自动继承传播，不阻止 DACL 内保留带有 `INHERITED_ACE` (`0x10`) 标志的 ACE（如 `D:P(A;OICI;FA;;;USER)(A;OICI;FA;;;SY)(A;OICIID;FA;;;WD)`），内核 `SeAccessCheck` 对这些 ACE 照常授权。对比同仓 `relay_tls_windows.rs:197` 与 `space_windows_policy.rs:37` 均明确拒绝任何带 `INHERITED_ACE` 的条目。此外 `permissions.rs:159-170` 先 `symlink_metadata(path)` 再通过路径调用 PowerShell `Get-Acl -LiteralPath`（存在链接替换 TOCTOU），且 `permissions.rs:231` 直接调用 `"powershell.exe"` 依赖 `PATH` 查找（应固定 `%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe` 或改用原生 Win32 句柄 ACL 校验）。
- **触发条件与影响**：携带受保护 NULL DACL 的 `.ma2a-downloads` 目录通过私密性校验，暴露下载暂存与锁文件给本机所有账户；携带 `INHERITED_ACE` 宽松授权的受保护状态目录、`keys/`、私钥文件、`state-v1.sqlite3` 或 `.ma2a-a2a-locks` 均绕过 `ma2a-store` ACL 校验。
- **修复与验收建议**：
  - `sddl::is_private` 显式校验 `flags` 仅由受支持的标志字母（`P`、`AR`、`AI`）组成且不包含 `NO_ACCESS_CONTROL`，并要求非空 `entries` 以 `')'` 结尾；在 `sddl::tests` 中加入 `O:BAD:PNO_ACCESS_CONTROL`、`O:BAD:PARNO_ACCESS_CONTROL`、`O:BAD:P(A;;FA;;;SY` 负面用例。
  - `permissions.rs:198` 移除 `Where-Object {!$_.IsInherited}`，改为若存在任意 `$_.IsInherited` 规则即返回非零失败（更好方案是统一改用基于已打开句柄的原生 `GetSecurityInfo` 校验，复用 `relay_tls_windows.rs` / `space_windows_policy.rs` 模式，同时消除 `powershell.exe` 路径与 TOCTOU 问题）。

---

#### F04 · [高] 第十轮 F03 修复不完整：Windows 竞态中间目录漏检，且 `ma2a-store` / `token_file` / `backup` 仍先继承父 ACL 再 `Set-Acl`
- **平台范围**：Windows（附 Unix 模式掩码小缺陷 `permissions.rs:144`）
- **位置**：
  - `crates/ma2a-runtime/src/ipc/windows.rs:42-62`
  - `crates/ma2a-store/src/permissions.rs:48-60, 96-100, 144`
  - 调用方：`crates/ma2a-app/src/a2a/token_file.rs:98, 175-183, 202-206`、`crates/ma2a-store/src/backup.rs:29-34, 92, 138-141`、`crates/ma2a-store/src/key_store.rs:164-172`
- **证据（静态核验）**：
  1. `ipc/windows.rs:46-61` 先用 `!level.exists()` 收集缺失层级，再逐级调用 `create_protected_dir(level)`。若某中间目录（如 `C:\shared\one`）在创建前被其他账户抢建为非私密目录或 NTFS 联接点，第 54 行匹配 `AlreadyExists` 后置 `created_leaf = false` 并**继续循环**；当最终叶目录 `state` 创建成功时，第 53 行又将 `created_leaf` 置为 `true`，第 59 行只对叶目录 `path` 执行保护/校验，被抢建的中间层级完全未做 ACL 或重解析点检查。此外 `!level.exists()` 会跟随链接并把 `PermissionDenied`/悬空重解析点折叠为 `false`。
  2. `ma2a-store/src/permissions.rs:55-59`（`create_new_private_dir`）与 `96-100`（`create_private_dir`）在第十轮未改为 `create_protected_dir`，仍执行 `fs::create_dir`/`fs::create_dir_all`（继承父目录 ACL）后再调用 PowerShell `apply_windows_acl`。受影响路径包括：`a2a/token_file.rs:98`（在用户指定的父目录创建 `.ma2a-a2a-locks`）、`backup.rs:92`（在备份目标父目录创建 `.ma2a-backup-*`，且 Windows 跳过了 `trusted_destination` 并在 `backup.rs:138-141` 仅检 `is_file()` 不比对文件身份）、以及不经 `ipc::windows::prepare` 的直接 `Repository::open` / `KeyStore::open`。
  3. 另注：`token_file.rs:202-206` 若 `create_protected_dir` 成功而 `protect_private_directory` 失败，未删除已创建的暂存目录；Unix 侧 `permissions.rs:144` 使用 `mode() & 0o777`（未像 `space_unix.rs:38` 那样用 `0o7777` 拦截 setuid/setgid/sticky 位）。
- **触发条件与影响**：在多用户 Windows 宿主机共享父目录下冷启动、启动 A2A 适配器或执行在线备份时，新建私密目录存在继承父 ACL 的时间窗口；并发抢建的中间目录/联接点可绕过校验。
- **修复与验收建议**：
  - 将 `create_protected_dir` 下沉至 `ma2a-store` 可复用层（或在 `ma2a-store/src/permissions.rs` 中直接以受保护安全描述符调用 `CreateDirectoryW`），使 `create_new_private_dir`、`create_private_dir` 与 `ipc/windows.rs` 均在创建瞬间附带受保护 DACL。
  - 在 `ipc/windows.rs:51-57` 中，每当任意层级返回 `AlreadyExists` 时立即对该层级本身做非重解析点与私密 ACL 校验（或拒绝继续）。
  - 在 Windows 原生测试中增加预创建非私密中间目录/联接点的负面回归测试，验证 `ensure_private_dir` 拒绝继续创建子目录。

---

#### F05 · [高] 未认证远端节点可永久耗尽全局 Control Sync 并发配额，阻断全节点撤销传播
- **平台范围**：Linux、macOS、Windows
- **位置**：
  - `crates/ma2a-net/src/control/handler.rs:89-93, 125-127, 131-150`
  - `crates/ma2a-net/src/control.rs:20-23`
- **证据（静态核验）**：
  1. `ControlHandler::accept` 在第 89 行（共享 Space 鉴权之前）获取 `_permit = self.limiter.try_acquire(peer)`（`CONTROL_PER_PEER_LIMIT = 2`、`CONTROL_GLOBAL_LIMIT = 16`）。
  2. 虽其余 I/O 均包裹了 `timeout(CONTROL_IO_TIMEOUT, ...)`，但第 125 行 `write_response(&mut send, result).await?` **没有任何超时**。即便对未授权连接（`result = Err(ControlRejection::Unauthorized)`），第 140 行仍调用无超时的 `send.write_all(&[status]).await`。
  3. 第 126 行 `let _closed = timeout(CONTROL_IO_TIMEOUT, connection.closed()).await;` 仍在 `_permit` 作用域内。
- **触发条件与影响**：任意未认证远端攻击者只需生成 8 个临时 Ed25519 密钥（每个开 2 条 `CONTROL_ALPN` 流 = 16），在 QUIC 传输参数中将 `initial_max_stream_data_bidi_local` 设为 `0`（或耗尽流控窗口并持续发送 QUIC PING 保活），即可让 16 个处理器永久阻塞在 `write_response` 中。即使使用默认窗口，不主动关闭连接也能持续占满 10 秒槽位。期间所有合法 Space 成员的入站控制同步与成员撤销更新均被 `try_acquire` 拒绝。
- **修复与验收建议**：
  - 为 `write_response` 添加短超时（例如 2–5 秒），并在 `write_response` 之前（最迟在响应写完后、等待 `connection.closed()` 之前）显式 `drop(_permit)`；对未授权连接直接 `connection.close(...)` 或不等待对端关闭。
  - 在 `ma2a-net` 增加回归测试：模拟未授权连接拒绝读取/流控窗口为 0 时，`ControlMetrics::active_streams` 在短超时内归零，合法成员仍可同步。

---

#### F06 · [高] 未认证远端通过 `EnrollmentHandler` + `biased select!` 饥饿整个 `Actor::run_loop`
- **平台范围**：Linux、macOS、Windows
- **位置**：
  - `crates/ma2a-net/src/enrollment.rs:54-129`
  - `crates/ma2a-runtime/src/actor/run.rs:45-48, 132-177`
  - `crates/ma2a-runtime/src/departure_actor.rs:130-154, 177`
- **证据（静态核验）**：
  1. `EnrollmentHandler` 注册在始终开放的 `ENROLLMENT_ALPN` 上，无任何单节点或全局并发限制器（对比 `ControlLimiter` / `EchoLimiter` / `ServiceLimiter`），将未认证来访直接送入 `self.enrollment_calls`。
  2. `Actor::run_loop`（`actor/run.rs:45-177`）采用 `tokio::select! { biased; ... }`，将 `self.enrollment_calls.recv()`（第 132 行）排在 `control_calls`（135）、`control_tasks`（138）、`echo_calls`（143）、`echo_tasks`（146）、`relay_observations`（151）、`control_rounds`（155）和 `periodic.tick()`（160）之前。
  3. `handle_enrollment_call` 在单线程 Actor 循环内串行执行；任意未认证远端发送 52 字节 `MLV1` 退出帧即触发异步 SQLite `load_space_chain`（`departure_actor.rs:177`），等待期间并发入站任务会持续填满 `self.enrollment_calls`。
- **触发条件与影响**：低带宽持续发送 52 字节未认证 `MLV1` 或畸形入网帧即可令 `enrollment_calls.recv()` 每轮均就绪；在 `biased;` 调度下，后续所有分支永远不会被轮询，彻底饿死控制面同步（含撤销投递）、Echo、中继观测更新与定时维护任务。
- **修复与验收建议**：
  - 为 `EnrollmentHandler` 增加与 `ControlLimiter` 类似的单节点/全局并发限流与轻量拒绝逻辑。
  - 移除 `actor/run.rs:46` 对外部网络入站通道的 `biased;` 优先饥饿（仅保留 `cancellation` 与必要本地控制命令优先，或对外部入站通道做公平轮询/独立任务派发）。
  - 增加单元测试：高频率向 `enrollment_calls` 灌入无效请求时，`control_calls` 与 `periodic` 分支仍在有界时间内得到调度。

---

#### F07 · [高] 未认证远端节点可耗尽全部 8 类流式服务全局并发限额并制造无界拒绝任务
- **平台范围**：Linux、macOS、Windows
- **位置**：
  - `crates/ma2a-net/src/service_protocol/handler.rs:41-58, 70-86`
  - `crates/ma2a-net/src/service_protocol/call.rs:130-147, 166-184`
  - `crates/ma2a-core/src/service/catalog.rs:87-97`
- **证据（静态核验）**：
  1. `ServiceHandler::admit`（`handler.rs:70`）在鉴权前获取 `ServicePermit` 并装入 `ServiceCall.stream._permit`。
  2. `StreamHost` 鉴权拒绝后调用 `call.reject(ServiceError::Unauthorized)`（`call.rs:166-169`），该函数将仍持有 `_permit` 的 `self.stream` 移入 `tokio::spawn(write_reject(self.stream, error))`。
  3. `write_reject`（`call.rs:172-184`）在 2 秒 `REJECT_WRITE_DEADLINE` 内执行 `write_all` 并等待 `stream.send.stopped().await`。未认证攻击者只要将流控窗口设为 0 或不回 ACK，即可让每条被拒流占住 `ServicePermit` 整整 2 秒。
  4. `catalog.rs:87-97` 中每类服务均满足 `global == 4 * per_peer`（如 `Transfer`/`Shell` 为 `(4, 16)`，`Exec` 为 `(8, 32)`，`Files`/`Process`/`Service` 为 `(16, 64)`），仅需 4 个随机 Ed25519 身份即可持续打满全部 8 类服务的全局上限。
  5. 限额满后，`handler.rs:48` 对后续每条流无上限地 `rejects.spawn(write_reject(...))`，可在连接流洪泛下累积大量 2 秒挂起任务。
- **触发条件与影响**：4 个未认证临时身份即可持续令全节点 8 类流式服务对合法 Space 成员返回 `ConcurrencyExceeded`，并堆积无界拒绝任务。
- **修复与验收建议**：
  - 在 `ServiceCall::reject` 移交后台写拒绝帧之前立即释放 `_permit`（及 `_active`），并对 `write_reject` 任务数量施加全局/单连接上限，超出上限时直接 `send.reset(...)` + `receive.stop(...)` 而不派发任务。
  - 增加并发测试验证未认证流拒绝不会占满合法成员的 `ServiceLimiter` 配额。

---

#### F08 · [高] `PrivateRelayServer` / `PrivateRelayAccess` 握手期撤销 TOCTOU 绕过，被撤销成员永久保留私密中继连接
- **平台范围**：Linux、macOS、Windows
- **位置**：
  - `crates/ma2a-net/src/relay_access.rs:35-61, 71-83`
  - `crates/ma2a-net/src/private_relay.rs:90-94`
  - 依赖核验：`iroh-relay 1.1.0` `src/server/http_server.rs:877-897`、`src/protos/handshake.rs:495-503`
- **证据（静态核验 + 依赖源码核验）**：
  1. 在 `iroh-relay 1.1.0` 中，`http_server.rs:877-879` 先调用 `authentication.authorize_with(&request, &self.access, &mut io).await?`。
  2. `handshake.rs:495-503` 在 `access_control.on_connect(request).await` 返回 `Access::Allow` 后，先执行异步网络写 `self.accept(io).await?`（发送 `ServerConfirmsAuth`），之后 `http_server.rs:896` 才执行 `self.clients.register(...)`。
  3. `PrivateRelayAccess::on_connect`（`relay_access.rs:72-83`）在返回 `Access::Allow` 时已将 `(endpoint_id, connection_id)` 插入 `state.connections`。若此时并发触发成员撤销并调用 `PrivateRelayServer::replace_from_spaces`（`private_relay.rs:90-94`），`relay_access.rs:51-58` 会把该连接从 `state.connections` **移除**并返回，`private_relay.rs:92` 调用 `self.clients.disconnect(endpoint_id, Some(connection_id))` 但因该客户端尚未注册到 `self.clients` 而静默无操作。
  4. 随后 `ServerConfirmsAuth` 写完，`http_server.rs:896` 正式将该已撤销成员注册进 `self.clients`。因 `(endpoint_id, connection_id)` 已被移出 `state.connections`，之后无论多少次 `replace_from_spaces` 都再也不会断开它。
- **触发条件与影响**：成员在被撤销瞬间正在完成中继握手（或有意放慢 TCP 读取拉长 `ServerConfirmsAuth` 发送窗口），即可永久留在私密中继上继续使用转发资源。
- **修复与验收建议**：
  - `replace_from_spaces` 检查 `self.clients.disconnect(...)` 返回值，或在 `PrivateRelayAccess` 中仅在 `on_disconnect` 时移出连接记录（将“待断开”集合保留并在后续刷新/短延迟重试断开，直到 `disconnect` 生效或收到 `on_disconnect`）；更彻底的做法是在 `replace_from_spaces` 中对所有不在 `allowed` 中的已知已连接 `endpoint_id` 不按单次 drain 清除，而是直到 `on_disconnect` 回调才清理。
  - 增加并发测试：模拟 `on_connect` 返回后、`register` 完成前执行 `replace_from_spaces`，验证连接最终必然被断开。

---

#### F09 · [高] 第十轮 F08 大面积遗漏：多处服务端帧写与出站流操作无超时无取消，可跨撤销永久占用并发槽位与资源
- **平台范围**：Linux、macOS、Windows
- **位置**：
  - `crates/ma2a-runtime/src/stream_host/inbound.rs:50-51, 138-139, 216, 230, 235, 238`
  - `crates/ma2a-services/src/proxy.rs:45`
  - `crates/ma2a-services/src/transfer/upload.rs:122`、`crates/ma2a-services/src/transfer/download.rs:88`
  - `crates/ma2a-runtime/src/processes/start.rs:106`、`crates/ma2a-runtime/src/processes/reads.rs:148`
  - `crates/ma2a-runtime/src/agents/host.rs:143, 181`、`crates/ma2a-runtime/src/agents/watch.rs:52`
  - `crates/ma2a-runtime/src/stream_plane/relay.rs:155-158, 188, 200-205, 253-264`
  - `crates/ma2a-net/src/service_protocol/client.rs:48, 62-65`
  - `crates/ma2a-runtime/src/ipc/server/stream.rs:189-190`
- **证据（静态核验）**：
  1. `docs/protocol/services-v1.md:255` 明确约定主机写开闭操作帧与关闭流最多等待 5 秒，但第十轮 `6d6f298` 仅覆盖 `exec.rs`、`shell.rs`、`reply.rs` 与 `processes/{attach,stdin,host}.rs`。
  2. **入站服务端遗漏**：
     - `inbound.rs:50-51, 138-139, 216, 230, 235, 238` 的 `writer.reject`/`writer.frame(Outcome)`/`writer.shutdown` 均无超时无取消；
     - `proxy.rs:45` 在已建立后端 `TcpStream`（第 39 行）后无超时无 `shutdown.cancelled()` 地写 `Accept`，挂起可跨撤销长期占用后端 TCP 套接字与 `Proxy` 槽位；
     - `transfer/upload.rs:122` 与 `transfer/download.rs:88` 写 `Accept` 无超时（且 `serve_transfer` 在 `inbound.rs:179-188` 无外层超时），未撤销慢节点可永久锁住 `Transfer` 槽位、打开的文件句柄及 `transfer_staging` 记录；
     - `processes/start.rs:106`（已在 DB 写入 `Created` 记录却尚未 `bring_up`）与 `processes/reads.rs:148` 未调用 `write_accept`；
     - `agents/host.rs:143, 181` 与 `agents/watch.rs:52` 的 `reject`/`Accept` 均无超时无取消。
  3. **中继与出站遗漏**：
     - `stream_plane/relay.rs:255` 转发终结帧 `Outcome`/`Reject` 未套用 `bounded(...)`（导致托管服务卡住 `max_calls` 槽位，出站 IPC 卡住 `Pending` 重放记录）；
     - `stream_plane/relay.rs:188, 200-205` 在 `upstream` 内向远端写数据与 `Cancel` 无超时，远端停读会阻止 `upstream` 返回，使 5 秒 `CANCEL_GRACE` 根本无法启动；
     - `ServiceClient::open`（`service_protocol/client.rs:48, 63`）调用 `connection.open_bi().await` 无超时，`ipc/server/stream.rs:189-190` 写 `Open` 帧无超时。
- **触发条件与影响**：扣留 QUIC 流控窗口的对端（或在出站路径上广告 `initial_max_streams_bidi = 0` 的恶意远端）可使上述任务无限挂起，耗尽 `ServicePermit`、后端套接字、托管服务 `max_calls`、本地 IPC `streams` 信号量与 `Pending` 重放槽位；其中 `inbound.rs`、`proxy.rs:45`、`agents/*`、`processes/{start,reads}` 在撤销授权后仍无法解除。
- **修复与验收建议**：
  - 所有入站 `Accept`/`Reject`/`Outcome`/`shutdown` 统一收口到带 5 秒超时与 `stop.cancelled()` 取消的共享辅助函数；
  - `ServiceClient::open` 的 `open_bi()`、`ipc/server/stream.rs:189-190` 的 `Open` 写入、`stream_plane/relay.rs` 的 `Cancel`/终结帧转发全部加入有界超时（并借鉴 `hosting/call/relay.rs` 在输入阻塞时独立启动取消宽限）。
  - 使用满缓冲 `tokio::io::duplex(1)`（无读端）对上述每条路径补齐超时回归测试，验证均在 5 秒宽限内结束并释放配额。

---

#### F10 · [高] `ConnectionManager` 活跃连接槽位（128）泄漏，导致全守护进程出站拨号失败
- **平台范围**：Linux、macOS、Windows
- **位置**：
  - `crates/ma2a-net/src/connection_manager.rs:186-233`
  - `crates/ma2a-net/src/control/protocol.rs:81-120`
  - `crates/ma2a-net/src/echo_protocol/client.rs:31, 48-63`
  - `crates/ma2a-net/src/service_protocol/client.rs:13, 47-68, 81-104`
- **证据（静态核验）**：
  1. `ConnectionManager::register`（`connection_manager.rs:193-206`）在 `active.len() == MAX_ACTIVE_CONNECTIONS`（128）时拒绝所有新出站连接，并向 `self.0.active` 与 observer 任务各存入一份 `Connection` 克隆；槽位仅在 `connection.path_events()` 结束（即 QUIC 连接真正关闭）时移除。
  2. 因此仅仅 `drop` 调用方持有的 `Connection` 句柄**不会**关闭 QUIC 连接（与 `service_protocol/client.rs:13` 注释假设相反）。
  3. 以下路径均跳过 `connection.close(...)`：
     - `ControlClient::exchange`（`control/protocol.rs:88-119`）：任意超时、读写失败或非 `0` 拒绝状态都在第 119 行之前提前返回；
     - `EchoClient::exchange`（`echo_protocol/client.rs:31, 48-63`）：外层 `ECHO_DEADLINE` 超时或第 62 行前出错直接丢弃 future；
     - `ServiceClient`（`service_protocol/client.rs:62-66, 81-104`）：`open_bi()` 失败、`forget()`、LRU 淘汰 `cache.pop_first()` 以及并发拨号覆写同键均从未调用 `connection.close`。
- **触发条件与影响**：多次失败/超时的控制或 Echo 拨号，或跨多节点/多服务轮换超过 32 个缓存项后（尤其对端保持 QUIC 保活时），`active` 累积至 128，此后守护进程全部出站控制同步、Echo 与流式服务拨号统一失败（`active connection bound reached`）。
- **修复与验收建议**：
  - 为 `ControlClient::exchange` 与 `EchoClient::exchange` 包装 RAII 连接关闭守卫（Drop 时自动 `connection.close(0, b"")`）；
  - `ServiceClient` 在 `open_bi()` 失败、缓存淘汰/覆写/`forget` 时确保无活跃流后关闭或由单次引用计数驱动关闭，且 `ConnectionManager.active` 改存弱句柄或由连接真正用途管理生命周期。
  - 增加单元测试：对超时/失败拨号及超过 `MAX_CACHED_CONNECTIONS` 的缓存淘汰循环 150 次后，确认 `ConnectionManager` 仍可继续拨号。

---

#### F11 · [高] 单一 Space 撤销成员永久中断该成员与撤销方在其他共享 Space 上的出站控制同步
- **平台范围**：Linux、macOS、Windows
- **位置**：
  - `crates/ma2a-runtime/src/control_sync.rs:143-170, 183-220`
  - `crates/ma2a-runtime/src/control_sync/exchange.rs:17-50`
- **证据（静态核验）**：
  1. 假设 Alice 与 Bob 共享 `Space 1` 与 `Space 2`，Alice 拥有 `Space 1` 并撤销了 Bob。Alice 的 `control_spaces_between(Alice, Bob)` 变为仅 `[Space 2]`，不再向 Bob 推送 `Space 1` 的撤销清单；若 `Space 1` 仅两人，Bob 永远无法得知自己被移除，本地仍认为共享 `[Space 1, Space 2]`。
  2. Bob 每轮控制同步通过 `prepare_round`（`control_sync.rs:159, 165-167`）将对 Alice 的全部本地共享 Space（`[Space 1, Space 2]`）打包进同一个 `ControlRequestV1`。
  3. Alice 的 `respond()`（`exchange.rs:26-41`）要求请求中的**每一个** cursor 和 push_page 均在 Alice 当前 `shared` 中获授权，遇 `Space 1` 即以 `ControlRejection::Unauthorized` 拒绝整包请求，`Space 2` 既不应用也不应答。
  4. 同理，`exchange.rs:43-49` 若某条 `push_pages` 中的清单正好把任一方移出某个 Space，`apply_pages` 提交后重算 `shared`（第 47 行），随后 `validate_cursor_spaces`（第 49 行）因该 Space 不再共享而整包返回 `ControlRejection::Invalid`，丢弃同批其余共享 Space 的应答。
- **触发条件与影响**：Bob 被 Alice 从 `Space 1` 撤销后，Bob 永远无法再向 Alice 推送 `Space 2` 的新地址记录、私密中继通告或 `Space 2` 的新清单/撤销；一旦 Bob 原地址过期（24 小时）或换 IP，Alice 也无法再拨通 Bob，`Space 2` 控制面永久断连。
- **修复与验收建议**：
  - 在 `exchange.rs` 中将多 Space 控制同步改为按 Space 粒度隔离处理（响应协议中支持单 Space 拒绝/跳过，或客户端在收到整批 `Unauthorized` 后回退为按单 Space 探测并隔离失权 Space）；对于 `exchange.rs:47-49`，在应用 `push_pages` 后仅对仍共享的 Space 生成应答页，或明确按 Space 报错。
  - 增加双 Space 集成测试：Alice 撤销 Bob 在 `Space 1` 的成员身份后，验证 Bob 仍能持续把 `Space 2` 的新清单与新地址同步给 Alice。

---

#### F12 · [高] `Files::Remove(recursive=true)` 无深度限制递归可致整个守护进程栈溢出崩溃或 FD 耗尽（并含 macOS APFS 边遍历边删漏项）
- **平台范围**：Linux、macOS、Windows（`readdir`+`unlink` 漏项主要影响 macOS APFS 及部分 Unix 文件系统）
- **位置**：
  - `crates/ma2a-services/src/files/dir_unix.rs:124-156`
  - `crates/ma2a-services/src/files/dir_windows.rs:28-31, 77-83, 146-180`
- **证据（静态核验）**：
  1. 虽然单次 `RemotePath` 限制 64 层，但持有 `FilesWrite` 权限的远端成员可通过重复 `mkdir` + `rename`（将 64 层子树移入另一 64 层子树之下）构造任意深度的目录树。
  2. `remove_tree` 采用直接函数递归，每一层都同时保持打开的父目录句柄与目录枚举器（Unix 上为 `Directory` + `at::Dir`；Windows 上 `Directory.held: Vec<Arc<File>>` 还在每次 `child()` 克隆整条祖先句柄列表，占用 $O(d)$ OS 句柄与 $O(d^2)$ 引用）且无最大深度上限。
  3. 在低 `RLIMIT_NOFILE` 环境（如 macOS 默认 256–1024）中，该操作可耗尽整个守护进程的文件描述符，令 QUIC/IPC/SQLite 全面出错；在高 `RLIMIT_NOFILE` 环境（如现代 Linux 默认 1,048,576）中，深递归将耗尽 Tokio 阻塞线程默认 2 MiB 栈并**直接触发进程 abort 崩溃**。
  4. 此外 `dir_unix.rs:127-151` 在单个 `at::Dir::read_from` 迭代中边读边 `remove_file`/`remove_dir`；在目录修改会导致 `readdir` 偏移失效/跳项的文件系统（典型如 macOS APFS、部分 FUSE/overlayfs）上，可能漏删目录项而令第 155 行 `remove_dir` 误报 `ServiceError::NotEmpty`。
- **触发条件与影响**：授权 `FilesWrite` 成员可远程打崩或耗尽整个守护进程；macOS 正常递归删除大目录时可能偶发误报 `NotEmpty`。
- **修复与验收建议**：
  - 将 `remove_tree` 改为显式堆栈迭代并设定合理的最大深度/同时持有句柄上界（超界返回 `ServiceError::TooDeep` 或 `InvalidPath`）；同时在 `mkdir`/`rename` 或删除中避免深层句柄 $O(d^2)$ 克隆。
  - 在 `dir_unix.rs` 中分批收集固定数量（如 256 条）子项名称并关闭目录流后再删除，循环直至目录为空（或在空目录判定前重扫确认），消除边读边删跳过条目问题。
  - 增加深度目录测试（验证超深目录受控拒绝而非崩溃）及大目录（>1000 项）递归删除测试。

---

#### F13 · [高] `Repository::persist_control_batch` 冲突更新漏置 `active = 1`，重新发布的私密中继通告永久不可见
- **平台范围**：Linux、macOS、Windows
- **位置**：
  - `crates/ma2a-store/src/control_batch.rs:115-123`
  - 对比：`crates/ma2a-store/src/relay_config.rs:259-267`、`crates/ma2a-store/src/control_state.rs:33-39`
- **证据（静态核验）**：
  - `relay_advertisement_state` 表含 `active INTEGER NOT NULL DEFAULT 1`，`reconcile_private_relay_activity` 停用记录时将其置为 `active = 0` 并保留高水位 `sequence`。
  - `relay_config.rs:259-267` 的单条提交 `ON CONFLICT DO UPDATE` 明确写入 `active = 1`。
  - 但控制同步与入网引导使用的 `persist_control_batch`（`control_batch.rs:115-123`）执行 `ON CONFLICT(space_id, relay_endpoint_id) DO UPDATE SET sequence = ..., issued_at_ms = ..., expires_at_ms = ..., advertisement_hash = ..., signed_advertisement = ...`，**漏掉了 `active = 1`**。
- **触发条件与影响**：任意曾被标记为 `active = 0` 的私密中继通告行，后续经控制同步收到更新的高序号合法通告时，高水位 `sequence` 被推进但 `active` 仍保持 `0`。`ControlSpaceState::active_relay_advertisements` 按 `is_active()` 过滤，导致该重新启用的私密中继通告永久不可见，且相同序号的后续重投被视为幂等跳过。
- **修复与验收建议**：
  - 在 `crates/ma2a-store/src/control_batch.rs:119-122` 的 `DO UPDATE SET` 中补上 `active = 1,`。
  - 在 `ma2a-store` 增加回归测试：先将某通告行停用（`active = 0`），再经 `persist_control_batch` 写入 `sequence + 1` 的新通告，断言 `active_relay_advertisements()` 返回该新通告。

---

#### F14 · [高] Agent 任务达 `MAX_DURABLE_EVENTS`（256）上限时将 `result` 写入 `Failed` 任务，且 `send_agent_message` 丢消息却误报成功并遗漏清理
- **平台范围**：Linux、macOS、Windows
- **位置**：
  - `crates/ma2a-store/src/agent_output.rs:136-154`
  - `crates/ma2a-store/src/agent_actions.rs:86-104, 233-253`
  - `crates/ma2a-runtime/src/agents/tasks.rs:108-116`
  - `crates/ma2a-runtime/src/agents/artifacts.rs:144-154`
- **证据（静态核验）**：
  1. `agent_output.rs:136-147` 处理 `State { state: Completed, message: Some(msg) }` 时，第 138-141 行先 `UPDATE agent_tasks SET result = ?2`，第 144 行再调用 `record_durable_event`。若任务已达 256 条持久化事件，`record_durable_event` 将任务转为 `Failed(Limit)` 并返回 `Ok(None)`，第 145 行直接返回 `Ok(true)` 提交事务。尽管第 150 行明确允许终态跳过事件上限，但携带结果消息的 `Completed` 反而失败，并在数据库中留下 `state = Failed` 却带有 `result` 的损坏记录（A2A 映射随后将其当作结果暴露）。
  2. `agent_actions.rs:99-103` 的 `send_agent_message` 在 `record_durable_event` 返回 `Ok(None)`（丢弃用户消息并将任务置为 `Failed(Limit)`）后，第 103 行仍返回 `AgentTaskChange::Changed`（对比 `add_artifact` 第 236 行返回 `Ended`），导致 `AgentManager::send`（`tasks.rs:112`）向调用方返回 `Ok(record)`。
  3. 无论 `AgentManager::send`（`tasks.rs:112`）还是 `AgentManager::publish`（`artifacts.rs:151`），在 `record_durable_event` 触发上限终止任务后均未调用 `self.task_ended(task)` 和 `self.assign(&agent).await`，导致该任务的 `proxy.open` 监听不关闭且排队任务不被调度。
  4. 另：`agent_actions.rs:250-252` 调用 `complete_call(...)` 丢弃了 `bool` 返回值，即便 `call` 已完成或不存在仍提交制品记录。
- **触发条件与影响**：长对话 Agent 任务在 256 事件边界完成或接收用户消息时出现状态损坏、静默丢消息、监听泄漏及队列阻塞。
- **修复与验收建议**：
  - `apply_state` 在 `state.is_terminal()` 时允许记录结果消息（或先判断是否超限，在失败分支绝不写入 `result`）；
  - `send_agent_message` 在 `record_durable_event` 返回 `None` 时返回 `AgentTaskChange::Ended`，并在 `AgentManager::send` 与 `AgentManager::publish` 观察到任务进入终态时调用 `self.task_ended(task)` 与 `self.assign(&agent).await`；检查 `complete_call` 返回 `true` 否则拒绝提交。
  - 增加 256 持久化事件边界单元测试覆盖 `Completed + message`、`send_agent_message` 和 `publish_agent_artifact`。

---

#### F15 · [高] `ma2a-agent` SDK 收到重复 `RuntimeToAgent::Task` 时覆写并销毁运行中任务的 `Slot`
- **平台范围**：Linux、macOS、Windows
- **位置**：
  - `crates/ma2a-agent/src/sdk.rs:180-196`
  - `crates/ma2a-agent/src/sdk/task.rs:159-174`
- **证据（静态核验）**：
  1. `sdk.rs:186-195`：
     ```rust
     let slot = Slot { inbox, cancel, calls: HashMap::new() };
     if shared.tasks().insert(task.clone(), slot).is_some() {
         return;
     }
     ```
     `HashMap::insert` 无条件用新的空 `slot` 覆盖并丢弃正在运行任务的旧 `Slot`，随后因 `is_some()` 直接 `return`，又立刻丢弃新 `slot` 对应的接收端。
  2. `sdk/task.rs:159-174` 的 `Task::call` 在 `emit` 前即把 `(call, answer)` 插入 `slot.calls`，若 `emit` 返回 `false` 或调用方在 `select!` 中取消该 future，条目直至整个任务结束前永不清理。
- **触发条件与影响**：任何重复的 `RuntimeToAgent::Task`（例如携带更高 `seq`）到达 SDK 时，原运行中任务的 `Slot` 立即被销毁：所有在途 `Task::call()` 立即以 `outcome_unknown` 失败、`Task::ask()` 收到通道关闭，且后续对该任务的 `Message`/`Cancel`/`ToolResult` 全部落入死亡替换槽。
- **修复与验收建议**：
  - `sdk.rs:193` 改为使用 `Entry::Vacant`（或先 `contains_key` 再插入），存在旧条目时直接返回而不触碰原 `Slot`；
  - `Task::call` 使用 Drop 守卫在返回或取消时移除未决 `slot.calls` 条目。
  - 在 `ma2a-agent` 增加回归测试：向活跃任务发送重复 `Task` 帧后，验证原任务仍正常收到后续 `Message` 与 `ToolResult`。

---

#### F16 · [高] A2A HTTP 服务（`ma2a a2a serve`）无连接数上限、无 HTTP 头部读取超时与正文读取超时
- **平台范围**：Linux、macOS、Windows
- **位置**：
  - `crates/ma2a-app/src/a2a.rs:91-94`
  - `crates/ma2a-app/src/a2a/http.rs:58-61, 140-144`
- **证据（静态核验）**：
  - `a2a.rs:91` 直接调用 `axum::serve(listener, http::router(shared))`，未设并发连接信号量、未限制 `http1_only()`、未设置 `header_read_timeout`（对比同仓 Web 控制台 `crates/ma2a-runtime/src/web/lifecycle.rs:149-195` 的 64 连接上限与 10 秒头部超时）。
  - `authenticate` 中间件（`http.rs:59, 76-90`）在 hyper 解析完请求头后才执行；认证后 `axum::body::to_bytes(body, MAX_REQUEST).await`（`http.rs:142`）也无超时限制。
- **触发条件与影响**：按 `a2a/token_file.rs:1-3` 的威胁模型，本机其他 OS 账户不可信。同机任意未认证进程向 `127.0.0.1:PORT` 建立大量 TCP 连接并保持慢速/空请求头（Slowloris），即可在 Bearer 认证前耗尽 A2A 适配器文件描述符与内存导致拒绝服务。
- **修复与验收建议**：
  - 复用 `web/lifecycle.rs` 的受限连接接收循环：增加并发连接信号量、瞬时 `accept` 错误退避、`http1_only()`、`header_read_timeout` 以及 JSON-RPC 正文读取超时（如 10 秒）。
  - 增加测试：打开超过上限的空闲慢连接后验证新合法请求在超时释放或配额保护下不受永久阻塞。

---

#### F17 · [高] 远端分页与帧读取循环缺少页数上限和游标单调性校验，恶意远端可致死循环或 OOM
- **平台范围**：Linux、macOS、Windows
- **位置**：
  - `crates/ma2a-app/src/a2a/list.rs:48-58`（`ListTasks` -> `all()`）
  - `crates/ma2a-app/src/a2a/remote.rs:122-135`（`card()`）
  - `crates/ma2a-app/src/commands/remote/files.rs:69-94`（`ma2a file ls`）
  - `crates/ma2a-app/src/commands/remote/hosted_stream.rs:52-86`（`ma2a process logs` -> `read_retained()`）
  - `crates/ma2a-service/src/caller.rs:145-156`（`Caller::describe`）
- **证据（静态核验）**：
  - 对比 `commands/remote/service.rs:64-100`（明确限制 `MAX_PAGES = 8`）与 `ma2a-services/src/client/service.rs:38-46`（拒绝意外帧）：
    1. `a2a/list.rs:48-58` 无限拉取远端 `tasks` 页并 `all.extend(tasks)`，既无页数/条数上限也不校验 `next > after`，恶意远端返回重复或递增假游标即可令 A2A 适配器死循环直至 OOM。
    2. `a2a/remote.rs:122-135` 无限循环 `AgentRequest::Agents(after.take())`，未校验 `next` 严格推进且无页数上限。
    3. `commands/remote/files.rs:69-94` 无限循环 `FilesRequest::List`，未校验 `next > previous_cursor` 且无页数上限。
    4. `commands/remote/hosted_stream.rs:80` 仅检查 `reached == Some(read.next)` 而非 `reached.is_some_and(|prev| read.next <= prev)`，恶意远端返回 `window.end = u64::MAX` 并在 `1` 与 `2` 间交替 `read.next` 即可绕过终止条件陷入死循环。
    5. `ma2a-service/src/caller.rs:145-156` 读帧循环以 `_ => {}` 静默忽略非终结帧，远端可持续发送 `Data`/`Control`/`Accept` 令调用永不返回。
- **触发条件与影响**：同 Space 的任意恶意或异常远端节点可将 A2A 适配器、CLI 客户端或 `ma2a-service` SDK 调用方锁死在无限循环中（A2A `ListTasks` 还可触发宿主 OOM）。
- **修复与验收建议**：
  - 对所有远端分页循环强制校验游标严格单调递增（`next > prev`）并设定有界最大页数/总量上限；
  - 将 `hosted_stream.rs:80` 改为 `reached.is_some_and(|prev| read.next <= prev)` 并限制单次保留日志轮数；
  - `Caller::describe` 将意外帧类型改为立即返回 `Err(Error::Protocol(...))`。
  - 为上述 5 处各补齐重复游标/倒退游标/非法帧单元测试。

---

#### F18 · [高] 守护进程 20 秒停机升级在阻塞任务卡死时于 `#[tokio::main]` `Runtime::drop` 永久挂起，已删套接字却仍持有 `SINGLETON_LOCK`
- **平台范围**：Linux、macOS、Windows
- **位置**：
  - `crates/ma2a-app/src/daemon.rs:25-42, 131-136, 288-300`
  - `crates/ma2a-app/src/main.rs:123-147`
- **证据（静态核验）**：
  - `daemon.rs:25-32` 注释明确把 `RUNTIME_SHUTDOWN_DEADLINE`（20 秒）设计为“Runtime 关不掉时强制退出进程、由 OS 回收 `static SINGLETON_LOCK`”的兜底机制。
  - 然而超时后 `shutdown_runtime`（`daemon.rs:294-298`）只是返回 `Err(TimedOut)`，第 133 行 `clean_up` 删掉 IPC 套接字和 `daemon.record`，随后把 `Err` 返回给标注 `#[tokio::main]` 的 `main()`（`main.rs:123`）。
  - `#[tokio::main]` 退出时隐式 `drop` Tokio `Runtime`，而 Tokio `Runtime::drop` 会无限期阻塞等待所有 `spawn_blocking` 线程退出（正是 `commands/remote.rs:104-119` 不得不改用 `std::process::exit` 规避的行为）。
- **触发条件与影响**：一旦有阻塞池任务卡死（如 FUSE/磁盘挂起、卡住的 SQLite 或 Windows PowerShell ACL 子进程）触发 20 秒停机超时，守护进程会先删光自己的套接字和记录文件，然后永远卡死在 `Runtime::drop` 中且不释放 `SINGLETON_LOCK`。`ma2a stop` 等满 60 秒失败，后续所有 `ma2a start`/`stop`/`restart` 均判为 `Unresponsive`，必须人工 `kill -9`。
- **修复与验收建议**：
  - 在 `daemon.rs` 停机超时且完成 `clean_up`、刷新标准错误日志后，直接调用 `std::process::exit(1)` 绕过 `Runtime::drop`（或手动构建 `tokio::runtime::Runtime` 并调用 `shutdown_timeout` 后退出）。
  - 增加测试：注入一个睡眠超过 20 秒的 `spawn_blocking` 任务并请求停机，验证守护进程在 `RUNTIME_SHUTDOWN_DEADLINE` 后数秒内真实退出并释放 `daemon.lock`。

---

### 中严重度（Medium）

#### F19 · [中] Web 登出与会话鉴权残留（第十轮 F07 遗留 + 多标签回归 + 空闲超时失效）
- **平台范围**：Linux、macOS、Windows（Web 前端 + Runtime HTTP 后端）
- **位置**：
  - `crates/ma2a-runtime/src/web.rs:94-101, 135-155, 179-186`
  - `crates/ma2a-store/src/session_mutations.rs:142-150`
  - `crates/ma2a-runtime/src/web/runtime_routes.rs:35-37, 114-122`
  - `crates/ma2a-runtime/src/web/runtime_routes/events.rs:54-56, 209-217`
  - `crates/ma2a-runtime/src/web/runtime_routes/agents.rs:40-41`
  - `web/src/api/web-auth.ts:61-84`、`web/src/app.tsx:36, 53-58, 107-127`
  - `web/src/api/mutations.ts:73-80`、`web/src/screens/use-services.ts:7, 18-48`
- **证据（静态核验）**：
  1. **F07 未闭环**：`session_mutations.rs:142-150` 仍将失效会话与 CSRF 不匹配合并返回 `Ok(None)`，`web.rs:151` 统一映射为 `403`；`app.tsx:116-122` 仍吞掉所有 `403` 并跳 `/login`。当 `ma2a_csrf` Cookie 缺失/重复、`valid_same_origin` 返回 `403` 或单次重试期间另一标签再次登录轮换会话时，界面仍显示已登出而 `HttpOnly` `ma2a_session` 依然有效。
  2. **读/SSE/SPA 路径漏修 `AuthFailure::Internal`**：`runtime_routes.rs:35-37, 120-122`、`events.rs:54-56, 209-217` 与 `web.rs:179-186` 仍将 `AuthFailure::Internal` 折叠为 `401`（或 `307 /login`），瞬时 Store 错误会触发 `RuntimeController` 踢回登录页。
  3. **多标签 CSRF 轮换破坏全部变更**：`app.tsx:36, 55` 仅在挂载时读取一次 `ma2a_csrf` 固化到 `RuntimeMutationClient`（`mutations.ts:77`）。标签 B 重新登录后覆写浏览器共享 Cookie，标签 A 的读流继续存活，但标签 A 的所有变更按钮——包括紧急 **“Revoke all sessions”**——均持续携带旧 CSRF 令牌并永久 `403`，直至手动刷新。
  4. **后台轮询击穿 15 分钟空闲超时**：`useServices`（`use-services.ts:7, 18-48`）每 3 秒发 5 个并行 GET；后端 `runtime_routes.rs:120` 调用写库续期的 `state.auth.authenticate(bearer)`（且 `agents.rs:40-41` 单请求调两次 `payload()`），使停留在 `/services` 的无人值守标签每 3 秒执行 6 次 SQLite `BEGIN IMMEDIATE` 写事务并不断延长 `idle_expires_at_ms`，彻底架空 15 分钟 `IDLE_TIMEOUT_MS`（直至 8 小时绝对上限）。
- **触发条件与影响**：见上述 4 条场景，直接影响 Web 登出一致性、应急撤销可用性、会话空闲超时策略与 Store 写负载。
- **修复与验收建议**：
  - 后端区分失效 Bearer（`401`）、CSRF/Origin 拒绝（`403`）与内部 Store 错误（`500`），所有读/写/SSE 路由一致；前端 `app.tsx` 仅将 `401` 视为已登出；
  - `RuntimeMutationClient` 每次请求动态读取最新 `currentWebSession()?.csrfToken`（或在 `403` 时按新 Cookie 重试）；
  - 后台轮询读接口（`/services`、`/operations`、`/processes`、`/agents`、`/hosted-services` 及 SSE 触发的快照刷新）改用只读 `state.auth.validate(&bearer)`，由显式用户交互触发 `session/touch`。
  - 补齐后端状态码单测（`401` vs `403` vs `500`）、前端双标签 CSRF 轮换测试及 `/services` 轮询不延长 `idle_expires_at_ms` 测试。

---

#### F20 · [中] 控制面同步批次脆弱性：零时钟偏差容忍、多节点并发旧序号误拒与 64-Space 单批预算饥饿
- **平台范围**：Linux、macOS、Windows
- **位置**：
  - `crates/ma2a-runtime/src/control_sync/scheduler.rs:40, 63-74`
  - `crates/ma2a-store/src/address_records.rs:81-83`、`crates/ma2a-store/src/relay_advertisement.rs:30-32`
  - `crates/ma2a-runtime/src/control_sync/pages/staging.rs:96-116, 168-179, 218-229`
  - `crates/ma2a-runtime/src/control_sync/pages.rs:51-129`、`crates/ma2a-runtime/src/control_sync.rs:166`
  - `crates/ma2a-runtime/src/enrollment_actor.rs:127-137`
- **证据（静态核验）**：
  1. **零时钟偏差 + 复用起始 `now_ms`**：`address_records.rs:81` 与 `relay_advertisement.rs:30` 拒绝任何 `issued_at_ms > now_ms` 的记录；而 `ControlRoundRunner`（`scheduler.rs:40, 70`）在 30 秒同步轮开始时捕获一次 `now_ms`，用于校验数秒后才返回的新记录。单条超前记录在 `staging.rs:102-116` 中直接以 `Invalid` 拒绝整批（连同新清单/撤销一起丢弃），在 `enrollment_actor.rs:127-137` 中则令 1 毫秒时钟偏差直接导致入网失败。
  2. **多节点并发拉取旧序号误拒**：`staging.rs:70-84` 对已应用的旧清单幂等跳过，但 `stage_address`（第 170 行）和 `stage_relay`（第 220 行）对任何 `sequence < current` 均返回 `Invalid`。同一轮并发拨号使用同一初始游标快照，节点 B 先返回成员 X 的 `seq = 7` 入库，1 毫秒后节点 C 按旧游标返回 X 的 `seq = 6` 及一份新撤销清单，节点 C 的整包响应被拒，撤销丢失。
  3. **64-Space 批次预算饥饿**：`control_sync.rs:166` 固定按 64 个 Space 打包并平均切分 `MAX_CONTROL_BATCH_BYTES`（`pages.rs:100-129`）。满 64 个 Space 时单页应答预算为 `(1_048_576 - 6 - 64*35) / 64 = 16,348` 字节（扣除 5 字节帧头剩 `16,343` 字节），小于合法最大地址记录 `MAX_ADDRESS_RECORD_LEN = 16,384`；推送请求还需先扣除 64 份游标字节，单页预算可跌至 ~6 KiB，小于单份 64 成员清单（~7.5 KiB），造成 `push_page` 永久截断。
- **触发条件与影响**：多节点并发同步、亚秒级 NTP 漂移或共享较多 Space 时，控制同步丢弃合法清单/撤销或陷入截断活锁。
- **修复与验收建议**：
  - 应用响应时重新取当前 `now_ms`，并给予合理的时钟漂移宽限（或仅忽略单条尚未来临/过期记录而不打翻合法清单）；
  - 将 `stage_address`/`stage_relay` 的已验证合法旧序号改为幂等忽略（仅对同序号不同内容保留分叉拒绝）；
  - 按实际预算动态分批 Space 数量，确保每页至少能容纳最大合法单条清单或地址记录。
  - 增加并发两节点顺序交错返回 `seq = 7` / `seq = 6 + 新清单` 的集成测试，以及 64-Space 大地址记录同步测试。

---

#### F21 · [中] 第十轮 F05 遗留：客户端 `Cancel` 不中断 `Files` 递归删除，`list`/`mkdir`/提交路径忽略 `Stop`，且取消上传泄漏 `transfer_staging`
- **平台范围**：Linux、macOS、Windows
- **位置**：
  - `crates/ma2a-services/src/files.rs:59-64, 88-104, 171`
  - `crates/ma2a-services/src/files/ops.rs:55-87, 110-119`
  - `crates/ma2a-runtime/src/stream_host/inbound.rs:179-188`
  - `crates/ma2a-services/src/transfer/upload.rs:88-102, 136-144`
- **证据（静态核验）**：
  1. `files.rs:59-62` 执行 `content_free`（含递归删除）时未与 `reply::client_cancelled(reader)` 并行 `select!`，客户端发送 `FrameKind::Cancel` 或重置流不会触发 `Stop`，直至 15 秒 `FILES_DEADLINE` 超时。
  2. `ops::list`（`ops.rs:55-87`）与 `ops::mkdir(parents=true)`（`ops.rs:110-119`）未接收或检查 `Stop`；被取消/撤销的超大目录列表仍扫描完整个目录并持续占用全局 16 个 `BLOCKING_JOBS` 槽位。
  3. 撤销触发 `stop.cancelled()` 丢弃 `serve_transfer`（`inbound.rs:179-188`）时，已派发到阻塞池的 `temporary.finalize(&destination, mode)`（`upload.rs:137`）和 `content::commit_write`（`files.rs:171`）均不检 `Stop`，会在撤销后继续把文件发布到目标路径；同时被丢弃的 `upload::serve` 跳过第 101 行 `journal.forget(&staging).await`，在重启前持续泄漏 SQLite `transfer_staging` 行。
- **触发条件与影响**：客户端取消无法中止远程递归删目录；撤销后在途上传/写入仍可能落盘；取消上传遗留数据库暂存日志行。
- **修复与验收建议**：
  - `files.rs:59-62` 使用 `tokio::select!` 并发监听 `reply::client_cancelled(reader)`；
  - 将 `Stop` 传入 `ops::list`（循环中检查）、`ops::mkdir`、`content::commit_write` 与 `TempFile::finalize`（发布重命名前最后一次检查 `stop.is_set()`）；
  - 在 `upload::serve` 中为 `journal.forget` 与临时文件清理增加防取消丢弃处理（例如在独立清理步骤或作用域退出时异步安排回收）。
  - 增加客户端发送 `Cancel` 立即终止 `Remove(recursive=true)` 及撤销后不提交在途上传的测试。

---

#### F22 · [中] `Actor::run_loop` 内联等待最长 30 秒的 `leave_space` 与 `redeem_enrollment`，阻塞全部控制面与本地命令
- **平台范围**：Linux、macOS、Windows
- **位置**：
  - `crates/ma2a-runtime/src/actor/run.rs:70-78`
  - `crates/ma2a-runtime/src/departure_actor.rs:79-83`
  - `crates/ma2a-runtime/src/enrollment_actor.rs:90-97`
  - `crates/ma2a-net/src/enrollment.rs:18`
- **证据（静态核验）**：
  - `Command::LeaveSpace`（`actor/run.rs:71`）与 `Command::RedeemEnrollment`（第 77 行）在单线程 `Actor::run_loop` 内直接 `.await` `exchange_enrollment` / `exchange_redemption`（总超时 `ENROLLMENT_EXCHANGE_TIMEOUT = 30s`）。
- **触发条件与影响**：对离线/失联 Space 拥有者执行退出或兑换邀请码期间，整个 Runtime Actor 冻结长达 30 秒，阻塞所有本地状态查询、`space revoke`、`graceful_shutdown` 及入站控制同步；两名互为拥有者的成员同时互相退出会直接死锁 30 秒。
- **修复与验收建议**：
  - 参照 `spawn_outbound_echo`（`actor/run.rs:124`）将网络往返阶段拆出到后台任务执行，仅在完成后回到 Actor 提交状态并广播事件。
  - 增加测试：向不可达地址发起 `leave_space` 或 `redeem_enrollment` 期间，并发 `handle.status()` 与入站控制同步仍在 1 秒内响应。

---

#### F23 · [中] torn 流在 `reset()` 前调用 `writer.shutdown()` 发出干净 QUIC `FIN`，破坏帧完整性不变量
- **平台范围**：Linux、macOS、Windows
- **位置**：
  - `crates/ma2a-services/src/reply.rs:56-75`
  - `crates/ma2a-runtime/src/hosting/call.rs:100-102`
  - `crates/ma2a-runtime/src/stream_host/inbound.rs:105-109`
  - 依赖核验：`noq 1.2.0` `src/send_stream.rs:346-348`
- **证据（静态核验 + 依赖源码核验）**：
  - `reply::outcome`（`reply.rs:61-69`）即使 `writer.frame` 因 `writer.is_torn()` 失败或中途超时导致流 torn，第 69 行仍无条件调用 `writer.shutdown()`；`hosting/call.rs:102` 在 `Terminal::Lost` 且 `writer.is_torn()` 时同样无条件调用 `writer.shutdown()`。
  - 核验 `noq 1.2.0` `src/send_stream.rs:346-348`：`poll_shutdown` 直接调用 `SendStream::finish()`，立即排队干净的 QUIC `FIN`。这使 `inbound.rs:105-109` 事后调用的 `reset(0)` 失效（对端已可收到截断帧 + 干净 `FIN`），且 `reply::outcome` 最坏停顿变成 `2 * FRAME_GRACE`（10 秒）。
- **触发条件与影响**：半截帧后被发送干净 `FIN`，违背 `inbound.rs:106-107` 声明的协议不变量，且延长终结超时。
- **修复与验收建议**：
  - 仅在 `!writer.is_torn() && sent.is_ok()` 时调用 `writer.shutdown()`；`reply::outcome` 对 `frame` 与 `shutdown` 共享一个 5 秒总截止时间。
  - 增加单元测试：在 torn `FrameWriter` 上调用 `reply::outcome` 后断言底层写入端未被 `poll_shutdown`。

---

#### F24 · [中] `shell/io.rs` 输出先关闭时 100% CPU 忙等；`processes/{attach,start}`、`hosting/call`、`agents/tools/proxy` 存在无界阻塞与任务泄漏
- **平台范围**：Linux、macOS、Windows（`hosting/call.rs:84-88` 在默认缓冲区较小的 macOS/Windows 上最易触发）
- **位置**：
  - `crates/ma2a-services/src/shell/io.rs:131-152`
  - `crates/ma2a-runtime/src/processes/attach.rs:177-229`
  - `crates/ma2a-runtime/src/processes/start.rs:162-172`
  - `crates/ma2a-runtime/src/hosting/call.rs:84-88`
  - `crates/ma2a-runtime/src/agents/tools/proxy.rs:44-83`
- **证据（静态核验）**：
  1. `shell/io.rs:144`：`chunk = output.recv() => chunk` 无 `if !output_closed` 守卫；当 PTY 输出先于 `child.wait()` 结束时，第 150-151 行设置 250ms `drain_until` 并 `continue`，此后每轮迭代立即返回 `None`，令工作线程以 100% CPU 空转最多 250ms。
  2. `processes/attach.rs:183, 191, 221-229`：`downstream` 写输出/控制帧无写超时；客户端发送 `Cancel` 或断读后，`upstream` 置 `detached = true` 退出，而 `downstream` 卡死在 `writer.data`，永久锁住 `ServicePermit`、`ReaderSlot`（上限 8）与 Process 唯一 `stdin` 租约。
  3. `processes/start.rs:162-172`：`open_watch` 读写 `Watch`/`Accept` 无超时（对比 `open_live` 的 10 秒超时）。
  4. `hosting/call.rs:84-88`：向本地托管服务 IPC 写入 `answer`（内含最大 8 KiB 的 `body`，JSON 编码后可达 11–16 KiB，超过 macOS 8 KiB Unix socket 与 Windows 4 KiB 命名管道默认发送缓冲）无超时无 `stop`/`service_stop` 取消，卡死的本地服务会跨撤销/停服永久锁死 `Slot` 与 `ServicePermit`。
  5. `agents/tools/proxy.rs:44-83`：第 75 行用裸 `tokio::spawn` 派发中继且未监听 `manager.0.shutdown`（停机泄漏）；第 69 行在 `accept` 循环内联 `call.open` 未 `select!` 取消信号；第 51-53 行对持续 `accept` 错误（如 `EMFILE`）无退避 100% 忙等。
- **触发条件与影响**：高 CPU 空转、附加流资源/stdin 租约永久占用、托管服务或代理任务在取消/停机后泄漏。
- **修复与验收建议**：
  - `shell/io.rs` 增加 `output_closed` 布尔守卫；
  - `processes/attach.rs` 为 `downstream` 写操作加超时并在 `detached` 后设 5 秒退出宽限；`open_watch` 与 `hosting/call.rs:84-88` 补上超时与取消监听；
  - `agents/tools/proxy.rs` 将子任务纳入 `manager.0.tasks`、同时 `select!` `shutdown`、并为 `accept` 错误添加退避。

---

#### F25 · [中] 第十轮 F15 遗留：`Files::List` 仍返回运行时/Windows 保留名；`remove_tree` 删除活跃上传暂存文件；Windows `remove_entry` 误报错误码
- **平台范围**：Linux、macOS、Windows（`remove_entry` 错误码仅 Windows）
- **位置**：
  - `crates/ma2a-services/src/files/ops.rs:62-65, 91-93`
  - `crates/ma2a-services/src/files/resolve.rs:50-81`
  - `crates/ma2a-services/src/files/dir_unix.rs:137-150`
  - `crates/ma2a-services/src/files/dir_windows.rs:127-134, 159-172`
- **证据（静态核验）**：
  1. `ops::addressable`（`ops.rs:91-93`）仅检查 `valid_component(name)`，未过滤 `resolve::is_runtime_reserved(name)` 及 `cfg!(windows) && resolve::is_windows_reserved(name)`，而 `resolve::check`（`resolve.rs:76`）拒绝所有这类名称。`Files::List` 因此仍返回 `.ma2a-downloads`、`.ma2a-upload-<req><nonce>.part`、`.ma2a-write-<nonce>.tmp` 及 Windows 保留名，既破坏 F15 “列出即可寻址”契约，又向只读成员暴露活跃上传的内部暂存文件名与实时大小。
  2. `dir_unix.rs:137` 与 `dir_windows.rs:159` 在 `remove_tree` 中只跳过 `is_private_directory`（`.ma2a-downloads`），未跳过 `.ma2a-upload-*` 和 `.ma2a-write-*`，并发 `rm -r` 可删掉正在进行的上传/小文件写入暂存文件。
  3. `dir_windows.rs:127-134` 在 `fs::remove_file` 返回任意 `PermissionDenied` 时无条件回退调用 `fs::remove_dir`。面对只读（`FILE_ATTRIBUTE_READONLY`）或无删除权限的普通文件，`RemoveDirectoryW` 返回 `ERROR_DIRECTORY`，对外误报 `ServiceError::NotADirectory` 而非 `PermissionDenied`。
- **触发条件与影响**：列目录返回不可寻址条目并泄露暂存名；并发递归删除破坏在途上传；Windows 删除只读文件返回误导性错误码。
- **修复与验收建议**：
  - `ops::addressable` 补上 `!resolve::is_runtime_reserved(name) && !(cfg!(windows) && resolve::is_windows_reserved(name))`；
  - `remove_tree` 跳过所有 `is_runtime_reserved` 条目（目录保留返回 `NotEmpty`）；
  - `dir_windows.rs:127-134` 仅对目录重解析点/符号链接回退 `remove_dir`，或保留原始 `PermissionDenied` 错误。
  - 在 `files_host.rs` 补充测试验证目录中存在 `.ma2a-downloads` 与 `.ma2a-upload-*.part` 时 `list` 不返回且 `remove_tree` 不删除它们。

---

#### F26 · [中] `AgentId` 允许 `"blobs"`（工作区与全局制品存储目录碰撞）及 Windows 设备名
- **平台范围**：Linux、macOS、Windows（`"blobs"`）；Windows（设备名）
- **位置**：
  - `crates/ma2a-runtime/src/agents.rs:191-199`
  - `crates/ma2a-core/src/service/agent_id.rs:26-40`
  - `crates/ma2a-runtime/src/agents/instance.rs:97-100, 116`
  - `crates/ma2a-runtime/src/agents/artifacts.rs:216-227`
- **证据（静态核验）**：
  - `AgentManager::workspace` 使用 `<state_dir>/agents-v1/<agent_id>/workspace`，而 `AgentManager::blobs` 使用 `<state_dir>/agents-v1/blobs`，两者共处同一父目录。
  - `AgentId::new`（`agent_id.rs:26-40`）接受 `"blobs"` 以及 Windows 保留设备名（`con`、`prn`、`aux`、`nul`、`com1`–`com9`、`lpt1`–`lpt9`）。
- **触发条件与影响**：定义名为 `"blobs"` 的 Agent 会把其工作区直接放进所有 Agent 共享的制品 Blob 目录内（默认工作目录上级即为全仓制品 Blob，且每 60 秒 `collect_blobs` 遍历时会反复尝试 `remove_file("workspace")`）。在 Windows 上定义名为 `con`/`nul`/`com1` 等的 Agent 定义阶段成功，启动时 `private_dir(&workspace)` 每次必败，陷入无限重启退避循环。
- **修复与验收建议**：
  - 将各 Agent 目录移入独立子目录 `<state_dir>/agents-v1/agents/<agent_id>/workspace`（与 `blobs/` 物理隔离），并在 `AgentId::new` 中拒绝 Windows 保留设备名（或将目录名编码/加前缀如 `agent-<id>`）。
  - 增加单元测试覆盖 `AgentId::new("blobs")` 与 `AgentId::new("nul")` / `AgentId::new("com1")`。

---

#### F27 · [中] Windows 命名管道名哈希未规范化 `state_dir`，大小写或斜杠差异令 CLI 无法控制守护进程
- **平台范围**：Windows
- **位置**：
  - `crates/ma2a-runtime/src/ipc/windows.rs:73-83`
  - `crates/ma2a-app/src/main.rs:154-158`
  - `crates/ma2a-app/src/daemon_control/state.rs:82-104`
- **证据（静态核验）**：
  - `ipc::windows::name` 直接对 `paths.state_dir()` 的原始 UTF-16 字节计算 `blake3` 派生管道名 `ma2a-<hex>`，未做规范化、大小写折叠或 `/` vs `\` 与尾随斜杠统一。
  - 而 `daemon_control::classify` 通过 NTFS 锁文件 `<state_dir>\run-v1\daemon.lock` 判断运行状态，NTFS 将 `C:\Users\Alice\state`、`c:\users\alice\state`、`C:\Users\Alice\state\`、`C:/Users/Alice/state` 视为同一目录。
- **触发条件与影响**：在 Windows 上用一种路径拼写启动守护进程，再从另一终端（如 PowerShell vs Git Bash vs cmd）用等价拼写执行 `ma2a status`/`stop`/`restart` 时，客户端检测到 `daemon.lock` 已加锁，却连向完全不同的管道名，误判运行中的守护进程为 `DaemonState::Unresponsive` 并拒绝操作。
- **修复与验收建议**：
  - 在 `IpcPaths::new` 或 `ipc::windows::name` 中对已存在的 `state_dir` 做规范化（或统一分隔符、去除冗余尾随斜杠并对 Windows 盘符/路径执行一致的大小写规范化；更好方案是优先取 `GetFinalPathNameByHandleW` / `canonicalize` 后再按 NTFS 规则折叠大小写）。
  - 在 Windows 原生测试中验证 `C:\...\state`、`c:\...\state\` 与 `C:/.../state` 派生出相同的命名管道名。

---

#### F28 · [中] 发布与质量门禁脚本缺口（第十轮 F01/F13/F17 遗留 + macOS Bash 3.2 数组空切片崩溃）
- **平台范围**：Linux、macOS、Windows（其中 `smoke-release.sh:98` 仅 macOS 默认 `/bin/bash` 3.2）
- **位置**：
  - `scripts/extract-runtime-references.pl:44, 48-58, 86, 105, 115-120, 122-155, 162-196`
  - `scripts/validate-release.sh:25, 30, 51-56`
  - `scripts/smoke-release.sh:90-98`
  - `xtask/src/loc.rs:78-81, 118-137`
- **证据（实测复现 + 静态核验）**：
  1. **`extract-runtime-references.pl`（实测复现，日志 `.omo/reviews/2026-11-25/agy/probe-js-all.log`，使用测试构造 JS 夹具）**：
     - 模板插值 `${fetch("https://evil.invalid/e")}`、模板内正则 `` `${a.replace(/'/g,"")}`;fetch("https://evil.invalid/d");const z="'"; ``、后置自增除法 `a++/2;fetch(...);b/2`、属性关键字除法 `obj.in/2;fetch(...);b/2`、`if(a)/"/.test(b);fetch(...)`、块后正则 `if(a){c()}/"/.test(b);fetch(...)`、`window.fetch` / `globalThis.fetch` / `new window.WebSocket` / 单参 `new URL` / `fetch?.(...)` 的扫描输出**全部为空**（漏报）；
     - 合法压缩代码 `if(a)/"/.test(b);const s="from 'bad:scheme'";` 输出 `asset	bad:scheme`（误报并阻断发版）；`from\`https://evil.invalid/tag\`` 误报为 `asset`；
     - 转义斜杠 `fetch("\/\/evil.invalid\/x")` 输出 `network	\/\/evil.invalid\/x`，绕过 `smoke-release.sh:136` 的 `//*` 拦截。
  2. **`validate-release.sh`（实测复现，日志 `.omo/reviews/2026-11-25/agy/probe-validate-release.log`，使用构造的软链 `.tar.xz` 测试夹具）**：
     - 归档中将 `ma2a` 设为符号链接 `ma2a -> /bin/sh` 时，`./scripts/validate-release.sh` 打印 `validated ... for x86_64-unknown-linux-gnu` 并返回 **`EXIT=0`**。原因：`tar -tJf` / `unzip -Z1` 不校验条目类型，且 `tr -d '\r'` 会抹平文件名中的 CR 字节，第 54 行还会忽略重复顶层目录条目。
  3. **`smoke-release.sh:98`（静态核验 + Bash 3.2 规范）**：
     - `smoke-release.sh:1-2` 启用 `set -euo pipefail`，第 90 行 `declare -a asset_queue=("index.html")`，首轮循环第 98 行执行 `asset_queue=("${asset_queue[@]:1}")`。在 macOS 系统自带 `/bin/bash`（GNU Bash 3.2.57）下，`set -u` 对长度为 1 的数组取 `:1` 空切片会直接报错 `asset_queue[@]: unbound variable` 退出（仅当环境提前在 `PATH` 中放入 Homebrew Bash 4.4+ 时才不触发）。
  4. **`xtask/src/loc.rs`（实测复现，日志 `.omo/reviews/2026-11-25/agy/probe-loc-extra.log` 及 `.omo/reviews/2026-11-25/agy/probe_loc2`，使用可经 `rustc --edition=2024` 编译的测试夹具）**：
     - 含 `let s = r#" " /* "#;` + 300 行代码的合法 Rust 文件返回 **`Ok(())`**（完全绕过 250 LOC 限制）；
     - 含 `let q = '"'; /*` + 260 行块注释的 5 行合法 Rust 文件返回 **`Err(265 pure LOC)`**（误报）；
     - 含 `#[allow(dead_code)] /*` + 260 行块注释 + 1 行函数的合法 Rust 文件（`rustc` 编译通过）返回 **`Err(262 pure LOC)`**（误报）；
     - 含多行模板 `` `hello\n/* ...\n` `` 或正则 `export const re = /a\/*/;` + 300 行代码的 TS 文件返回 **`Ok(())`**（绕过）。
- **触发条件与影响**：依赖压缩产物变动可能误阻断发版，或外部引用/软链归档/超长源文件绕过门禁；无 Homebrew Bash 的 macOS 本地执行 `smoke-release.sh` 直接崩溃。
- **修复与验收建议**：
  - JS 扫描改用已有前端工具链（`bun` 生态中的真实 JS AST 解析器如 Acorn/SWC/oxc 或严格词法器）替换正则拼凑的 Perl 脚本，并反转义字符串字面量；
  - `validate-release.sh` 解析 `tar -tvJf` / `zipinfo -l` 强制所有载荷条目为普通文件、目录条目仅限单一顶层目录且无控制字符/CR；
  - `smoke-release.sh:98` 改为基于索引指针（`queue_index=0; while (( queue_index < ${#asset_queue[@]} )); do current=${asset_queue[queue_index]}; queue_index=$((queue_index + 1)); ...`），彻底兼容 Bash 3.2；
  - `xtask/src/loc.rs` 维护跨行字面量状态机，正确识别 Rust 字符字面量、原始字符串、属性行尾注释与 JS/TS 正则/模板字符串，并将上述 5 组用例固化进 `xtask` 与 `scripts/test-release-failures.sh`。

---

#### F29 · [中] 续传身份校验、邀请文件发布、本地安全打开及 A2A/代理运维缺口
- **平台范围**：标注于各子项
- **位置与证据（静态核验）**：
  1. **续传身份不匹配覆写 sidecar + 上传漏检 `result.bytes`**（全平台；`crates/ma2a-services/src/client/resume.rs:153-172`、`crates/ma2a-app/src/commands/remote/transfer.rs:19, 27, 83-90`、`crates/ma2a-services/src/client/transfer.rs:141-145`）：续传 `offset > 0` 时若异常远端忽略 `expected` 并以新 `IdentityB` 接受，`Partial::begin(identity)` 直接覆写 sidecar 而不拒绝或截断，流中断后保留被混合内容污染的 partial；`file get` 用未规范化 CLI 路径拼 sidecar 键导致等价路径丢失续传进度；`client::transfer::upload` 未校验 `result.bytes == size`。
  2. **邀请文件 Windows `\\?\` 拼接与 Unix 非暂存写**（Windows/Unix；`crates/ma2a-runtime/src/ipc/server/execute/invite_file.rs:7-28, 34-76`）：Windows 端将未校验的 `name` 拼接到 `pin_directory` 返回的 `\\?\` 路径，尾随点/空格或 `CON`/`NUL` 会创建标准 Win32 难以管理的逐字文件，含 `:` 名称先消耗已提交邀请再在重命名失败；Unix 端 `write_all` 失败不清理已创建的半截文件且未 `sync_all`。
  3. **Unix TLS 密钥打开未设 `O_NONBLOCK`**（Linux/macOS；`crates/ma2a-net/src/relay_tls.rs:180-194`）：`open_without_following_links` 缺少 `OFlags::NONBLOCK`，若配置路径指向 FIFO 会阻塞在 `open()` 之前，无法到达 `metadata.is_file()` 检查。
  4. **A2A token 清理无界读且跟随软链**（全平台；`crates/ma2a-app/src/a2a/token_file.rs:229-233`）：`remove_if_ours` 直接调用 `std::fs::read(path)`，若被替换为软链/FIFO/大文件可能跟随软链、阻塞停机或无界读入内存。
  5. **Windows `.ma2a-downloads/locks` 未复检私密 ACL**（Windows；`crates/ma2a-services/src/client/resume/helpers.rs:85-98`）：仅检 `.ma2a-downloads` 的 `is_private()`，未检已存在的 `locks` 子目录；因 Windows 所有用户默认拥有 `SeChangeNotifyPrivilege`（绕过遍历检查），非私密 `locks` 子目录仍可按完整路径直接访问。
  6. **A2A 远端制品流与 SSE 启动期断连不取消后台任务**（全平台；`crates/ma2a-app/src/a2a/artifact.rs:78-91, 129-147`、`crates/ma2a-app/src/a2a/stream.rs:152-163`）：`artifact.rs:81-86` 未监听 `outcome.closed()`，`stream.rs:152-163` 首包拉取阶段未监听 `frames.closed()`，HTTP 客户端断开后挂起的远端流继续占用槽位；`artifact.rs:137-142` 累加 `offset` 未做溢出与单块越界校验。
  7. **`ma2a proxy forward` 瞬时 `accept` 错误即退出且无并发上限**（全平台；`crates/ma2a-app/src/commands/remote/proxy.rs:58-75`）：`listener.accept().await?` 遇单次 `ECONNABORTED`/`EMFILE` 直接终止转发进程，且无并发信号量限制 `tokio::spawn`。
- **修复与验收建议**：
  - `Partial::begin` 在已有不同 `self.identity` 时返回 `PreconditionFailed`/`IntegrityMismatch`；`file get` 以规范 `RemotePath::to_string()` 作为 sidecar 源键；`upload` 补检 `result.bytes == size`；
  - `invite_file::check_destination` 拒绝目录尾斜杠、`:`、及 Windows 保留名/尾随点空格，Unix 侧改用暂存写+失败清理；
  - `relay_tls.rs:189` 增加 `OFlags::NONBLOCK`；`token_file::remove_if_ours` 改为 `NOFOLLOW`/`NONBLOCK` + 有界读取（如 `token.len() + 1`）；`helpers.rs:94` 对 `locks` 增加 `locks.is_private()?` 校验；
  - `a2a/artifact.rs` 与 `a2a/stream.rs` 对远端拉取全程 `select!` `closed()` 并对齐 `agent_artifact.rs` 的 `checked_add`/越界校验；`proxy.rs` 增加 `accept` 退避与并发上限。

---

### 低严重度（Low）

#### F30 · [低] 第十轮 F16 遗留：`xtask/src/unsafe_scan.rs` 未识别 Rust 合法 `Pattern_White_Space`，可绕过 `unsafe` 与 `#[allow(unsafe_code)]` 检测
- **平台范围**：Linux、macOS、Windows
- **位置**：`xtask/src/unsafe_scan.rs:39-41, 75-83, 168-181`
- **证据（实测复现）**：
  - `scan_unsafe_tokens` 第 39 行仅跳过 `ch.is_ascii_whitespace()`，而 Rust 词法允许 `Pattern_White_Space`（`\u{000B}`、`\u{0085}`、`\u{200E}`、`\u{200F}`、`\u{2028}`、`\u{2029}`）；`skip_raw_ident` 第 174-179 行则消费全部 `!c.is_ascii()` 字节，直接跨越非 ASCII 空白吞掉后续 `unsafe` 关键字。
  - 使用独立探针 `.omo/reviews/2026-11-25/agy/probe_unsafe.rs` 与 `probe_unsafe_compile.rs`（测试用构造源码）实测：
    - `macro_rules! m { ($i:ident $b:expr) => { $b }; } fn f() { m!(r#x\u{200E}unsafe { () }); }` 经 `rustc --edition=2024` 编译通过且包含真实 `unsafe` 块，而 `scan_unsafe_tokens` 返回 `(false, false)`；
    - `#[allow\u{200E}(unsafe_code)] fn g() {}` 经 `rustc --edition=2024` 编译通过，而 `scan_unsafe_tokens` 返回 `(false, false)`。
- **触发条件与影响**：在源码中使用 Rust 合法的非 ASCII 空白字符可逃避仓库内 `unsafe_code` 审计测试。
- **修复与验收建议**：
  - 统一实现 `is_rust_whitespace(ch: char)`（覆盖 ASCII 空白、`\u{000B}`、`\u{0085}`、`\u{200E}`、`\u{200F}`、`\u{2028}`、`\u{2029}`），在 `scan_unsafe_tokens` 与 `skip_raw_ident` 中按 `char` 迭代并在遇到非标识符字符/空白时停止。
  - 将上述两例加入 `xtask/src/unsafe_scan.rs` 单测并要求分别返回 `(true, false)` 与 `(false, true)`。

---

#### F31 · [低] 发版元数据路径泄漏/校验松弛、`xtask` 策略边界及 Web 头部/登录页细节
- **平台范围**：Linux、macOS、Windows
- **位置**：
  - `scripts/generate-license-report.sh:14-18`、`scripts/generate-sbom.sh:80, 100`、`scripts/validate-release-metadata.sh:16-21`、`.github/workflows/release.yml:160, 186`
  - `xtask/src/pins.rs:147-155, 185-211`、`xtask/src/release.rs:150-168`
  - `crates/ma2a-runtime/src/web/headers.rs:48-54`、`crates/ma2a-runtime/src/web/lifecycle.rs:135-141`、`web/src/app.tsx:36, 79-96`
- **证据（实测复现 + 静态核验）**：
  1. **发布产物含构建机绝对路径（实测复现）**：运行 `(cd web && bun pm licenses --json --prod) | jq 'to_entries[0].value[0]'` 实测输出包含 `"paths": ["~/.../ma2a/web/node_modules/ky"]`（实际为完整构建机绝对路径）。`generate-license-report.sh:14-18` 原样写入 `<archive>.licenses.json` 并由 `release.yml:160,186` 随 GitHub Release 公开，既泄露本地路径又破坏路径无关可复现性。
  2. **SBOM 边界（静态核验）**：`generate-sbom.sh:80` 将 Rust 许可证中的 `/` 规范化为 ` OR `，但第 100 行前端依赖未做同样规范化（一旦引入 `MIT/Apache-2.0` 前端包即失败）；`validate-release-metadata.sh:16-20` 未断言 `SPDXID` 和 `documentDescribes[0]` 为非 `null` 字符串。
  3. **`xtask` 策略松弛（静态核验）**：`pins.rs:185-211` 未拒绝同时含 `git` 与 `version = "=..."` 的依赖且不检查 `[patch.*]`；`release.rs:150-168` 的 `string_set` 将缺失或非数组字段折叠为空集并丢弃非字符串元素，使删掉 `installers`/`ci`/`hosting` 字段也能通过 `exact_array(..., [])`。
  4. **Web 细节（静态核验）**：`headers.rs:52-53` 对 `state.config.binding.is_some()` 一律豁免缺失 `Sec-Fetch-Site`，但生产 `ManagedWebServer::bind_at`（`lifecycle.rs:135-141`）始终设置 `binding: Some(...)`，导致环回 `127.0.0.1` 同样不强制检查缺失的 `Sec-Fetch-Site`（与第 50-51 行注释不符，仍受 `Origin` 保护）；`app.tsx:80` 在访问 `/login` 时只要残留 `ma2a_csrf` Cookie 仍会启动 `RuntimeController` 请求 `/api/v1/snapshot` 与 `/api/v1/events`。
- **触发条件与影响**：发版 `.licenses.json` 暴露宿主机路径；策略脚本漏检配置漂移；环回 Web 请求跳过缺失 Fetch Metadata 检查及 `/login` 冗余请求。
- **修复与验收建议**：
  - `generate-license-report.sh` 用 `jq` 剔除或相对化前端条目中的 `paths`；`generate-sbom.sh:100` 同步规范化 `/` -> ` OR `；`validate-release-metadata.sh` 断言 `SPDXID` 为非空字符串且不等于 `"SPDXRef-DOCUMENT"`；
  - `pins.rs` 拒绝 `git` 键并扫描 `[patch.*]`；`release.rs` 显式校验字段存在且全部元素无重复为字符串；
  - `headers.rs:53` 改为 `state.config.binding.as_ref().is_some_and(|b| !b.address.ip().is_loopback())`；`app.tsx:80` 排除 `/login` 与 `/setup` 路由。

---

## 第三部分：独立执行的验证命令、探针与基线结果

所有测试均在独立输出目录 `CARGO_TARGET_DIR=.omo/reviews/2026-11-25/agy/target` 下执行，未触碰仓库源码或默认 `target/`。探针中使用的合成 JS、合成 Rust 源文件、软链 `.tar.xz` 归档及单二进制测试归档均为**仅用于验证的测试夹具**。

| 验证项 | 执行命令（可复制） | 结果与日志 |
| --- | --- | --- |
| 格式检查 | `cargo fmt --all -- --check` | **通过**（退出码 `0`） |
| 第十轮脚本与 e2e 回归测试 | `./scripts/test-runtime-references.sh && ./scripts/test-e2e-output-reuse.sh` | **通过**（退出码 `0`） |
| 前端单测 / 类型 / Biome | `(cd web && bun run test && bun run tsc --noEmit && bun run biome check .)` | **通过**（24 个测试文件 / 155 条用例通过，118 文件检查通过；`.omo/reviews/2026-11-25/agy/web-checks.log`） |
| `xtask` 全套检查 + Windows GNU Clippy | `CARGO_TARGET_DIR=.omo/reviews/2026-11-25/agy/target cargo run --locked -p xtask -- check-loc && CARGO_TARGET_DIR=.omo/reviews/2026-11-25/agy/target cargo run --locked -p xtask -- check-pins && CARGO_TARGET_DIR=.omo/reviews/2026-11-25/agy/target cargo run --locked -p xtask -- check-support && CARGO_TARGET_DIR=.omo/reviews/2026-11-25/agy/target cargo run --locked -p xtask -- check-release && CARGO_TARGET_DIR=.omo/reviews/2026-11-25/agy/target cargo test --locked -p xtask && CARGO_TARGET_DIR=.omo/reviews/2026-11-25/agy/target CFLAGS_x86_64_pc_windows_gnu=-D_WIN32_WINNT=0x0601 cargo clippy --locked --target x86_64-pc-windows-gnu --workspace --all-targets --all-features -- -D warnings` | **全部通过**（退出码 `0`；`.omo/reviews/2026-11-25/agy/cargo-gates.log`） |
| Rust 核心单测与集成测试 | `CARGO_TARGET_DIR=.omo/reviews/2026-11-25/agy/target cargo test --locked -p ma2a-core -p ma2a-store -p ma2a-services -p ma2a-net -p ma2a-runtime -p ma2a-agent -p ma2a-service --lib && CARGO_TARGET_DIR=.omo/reviews/2026-11-25/agy/target cargo test --locked -p ma2a-services --test files_host --test client_transfer` | **全部通过**（`ma2a-store` 6、`ma2a-services` 17+15、`ma2a-runtime` 200、`ma2a-service` 5 等全过；`.omo/reviews/2026-11-25/agy/{unit-tests,net-runtime-tests,app-sdk-tests,core-tests}.log`） |
| **F01 实测探针**：`.woff2` 触发 `smoke-release.sh` 失败 | `CARGO_TARGET_DIR=.omo/reviews/2026-11-25/agy/target cargo build --locked -p ma2a-app && ./scripts/smoke-release.sh .omo/reviews/2026-11-25/agy/smoke-probe/test.tar.xz x86_64-unknown-linux-gnu` | **复现失败**（`SMOKE_EXIT=1`，`returned unexpected content type font/woff2`；`.omo/reviews/2026-11-25/agy/probe-smoke-woff2.log`） |
| **F03 实测探针**：`sddl::is_private` 接受 `D:PNO_ACCESS_CONTROL` | `rustc --edition=2024 .omo/reviews/2026-11-25/agy/probe_sddl.rs -o .omo/reviews/2026-11-25/agy/probe_sddl && .omo/reviews/2026-11-25/agy/probe_sddl` | **复现绕过**（3 组 `D:P*NO_ACCESS_CONTROL` 与未闭合 ACE 均返回 `true`；`.omo/reviews/2026-11-25/agy/probe-sddl.log`） |
| **F28 实测探针**：JS 运行时引用扫描漏报/误报 | `bash .omo/reviews/2026-11-25/agy/probe-js-all.sh` | **复现**（7 组漏报输出为空，2 组误报输出 `asset`，1 组转义绕过；`.omo/reviews/2026-11-25/agy/probe-js-all.log`） |
| **F28 实测探针**：`validate-release.sh` 放行软链归档 | `bash .omo/reviews/2026-11-25/agy/probe-validate-release.sh` | **复现**（含 `ma2a -> /bin/sh` 软链归档以 `EXIT=0` 通过；`.omo/reviews/2026-11-25/agy/probe-validate-release.log`） |
| **F28 实测探针**：`loc.rs` 绕过与误报 | `.omo/reviews/2026-11-25/agy/probe_loc2 && .omo/reviews/2026-11-25/agy/probe_loc_arg .omo/reviews/2026-11-25/agy/loc_cases2/ts_regex_bypass .omo/reviews/2026-11-25/agy/loc_cases2/rs_attr_fp` | **复现**（原始字符串/模板/TS 正则 300+ LOC 返回 `Ok(())`，合法 Rust 字符字面量与属性行注释误报 `Err`；`.omo/reviews/2026-11-25/agy/probe-loc-extra.log`） |
| **F30 实测探针**：`unsafe_scan.rs` Unicode 空白绕过 | `rustc --edition=2024 --crate-type=lib .omo/reviews/2026-11-25/agy/probe_unsafe_compile.rs -o /tmp/probe_unsafe_compile.rlib && .omo/reviews/2026-11-25/agy/probe_unsafe` | **复现**（`rustc` 编译通过的 `r#x\u{200E}unsafe` 与 `#[allow\u{200E}(unsafe_code)]` 均返回 `(false, false)`） |
| **F31 实测探针**：前端许可证报告泄露绝对路径 | `(cd web && bun pm licenses --json --prod) \| jq 'to_entries[0].value[0].paths'` | **复现**（包含宿主机完整绝对路径） |
