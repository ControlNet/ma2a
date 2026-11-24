# MA2A 第十轮代码审查

基线为 `master`、`HEAD` 和本地 `origin/master` 共同指向的 `c4f72a2`。范围 `19a42fb..c4f72a2`，11 个提交，110 个文件，新增 2583 行、删除 737 行。CI 全绿是本轮给定的背景。以下结果来自该提交的工作树、重新构建的测试和独立探针。位置均相对仓库根目录。

**最先需要处理的是发布冒烟和上传清扫。** 实际前端产物会被发布脚本当作非法资源而拒绝。另一个问题只需一次已正常返回的上传冲突和一次正常重启，就可能删除一个此次上传从未创建的文件。Windows 冷启动、工作区路径和撤权还有只修到部分调用路径的情况。以下 F 编号均属本轮，验收栏描述修复后应满足的条件。

## 本轮发现

### F01 · 高 · 正常前端产物过不了发布冒烟

**位置与证据（实测）。** `scripts/extract-runtime-references.pl:38–41` 在整段 JS 中搜索 `from` 加空白和引号。`web/src/screens/echo.tsx:74` 的界面文案以“reply from ”结束。扫描器把该字符串的结束反引号当成模块路径的开始，读出后续含冒号的 JS。用本轮二进制、实际前端和项目文档组装 Linux 归档后，文件清单和摘要检查通过，`scripts/smoke-release.sh:152–157` 拒绝该引用并退出 1。用 Bun 1.3.5 重建的入口 JS 与嵌入版本逐字节相同。仓库现有的资源扫描测试和发布负例均通过。

**触发与影响。** 正常构建即可触发，不需要更改归档。发布工作流在签证前运行这一步，因而普通分支 CI 全绿仍可能无法发布。更改 CSS 正则没有解决 JS 文案的误判。

**修复与验收。** 使用 JS 词法或语法信息识别真正的导入和资源表达式。在发布校验中先对当前生产构建运行完整正向冒烟，再跑外部资源负例。两者分别应成功和明确拒绝，拒绝原因应指向实际的外部引用。

### F02 · 高 · 已拒绝的上传会把既有文件留在重启清扫名单中

**位置与证据（静态，第八轮 F10 的影响扩大）。** `crates/ma2a-services/src/transfer/upload.rs:80–99` 先记日志，再独占创建暂存文件，遇到 `AlreadyExists` 时保留日志。`crates/ma2a-store/src/transfer_staging.rs:32–51` 按路径插入或替换，无法区分“之前的上传已登记”和“此次第一次登记但创建失败”。`crates/ma2a-runtime/src/store/services.rs:162–176` 启动时按名称删除，入口在 `crates/ma2a-runtime/src/lifecycle.rs:88`。

**触发与影响。** 目标旁恰有符合暂存命名规则的本地文件，原先没有对应日志。上传正常返回冲突后，该文件仍在，下一次正常启动却会清扫它。原文档重点描述崩溃、恢复或目录移动，本例三者都不需要。用已有日志保护并发上传的修复动机成立，问题出在对两种同名来源作了相同处置。

**修复与验收。** 让日志区分原有认领与本次尝试，并把清扫资格绑定到已成功创建的对象。分别覆盖运行中的同名上传、无日志的既有条目和创建中断。前者的日志应保留，第二种在冲突及正常重启后应逐字节保留，第三种只回收本次拥有的暂存物。

### F03 · 高 · Windows 首次启动的递归创建路径仍先继承 ACL

**位置与证据（静态，Windows）。** `crates/ma2a-runtime/src/ipc/windows.rs:36–48` 只有在父目录存在且受保护的创建成功时才走原子创建分支。父级尚不存在或创建出错都会转入 Store。`crates/ma2a-store/src/permissions.rs:5–11,96–100` 的 Windows 分支先调用普通 `create_dir_all`，再按路径给最终目录应用 ACL。

**触发与影响。** 首次使用含缺失父级的状态目录时，创建期间的访问权依赖上级目录的可继承规则。新增的中间目录也不经过同等检查。在允许其他账户修改子项的父目录下，最终叶子的权限检查无法证明创建全程私有或名称稳定。没有观察到实际凭据泄露。

**修复与验收。** 区分同名竞争和其他创建错误。从经过验证的父目录逐级创建，首次对外可见时即带正确安全描述符。原生 Windows 验收应覆盖已有父级、多级缺失、并发创建和创建失败，并检查各级目录及启动结果。

### F04 · 高 · 工作区路径检查未绑定到工具实际读写的目录

**位置与证据（静态）。** `crates/ma2a-runtime/src/agents/tools/workspace.rs:73–91,98–119` 读操作返回规范路径，写操作逐段检查后返回路径名。`crates/ma2a-runtime/src/agents/tools/files.rs` 的 `download` 在远端接受后重新检查，随后由 `Partial::open` 在另一阻塞任务中重新打开父目录。`client/resume/helpers.rs` 的 `Helpers::open` 从当时解析到的父目录开始固定。读侧还允许工作区内的链接，而解析完成后只验证工作区前缀，没有再检查解析结果中的下载保留目录。

**触发与影响。** 工作区在检查和实际打开之间发生目录轮换时，下载可能固定并替换错误目录下的同名目标。读工具也可能通过工作区内的普通别名读到原本禁止直接指定的辅助文件。受影响的是工具承诺的路径约束，同账户 Agent 程序本身已有操作系统权限。另一个平台边界是第 41–68 行的设备名表遗漏 Windows 认可的上标数字设备名。[Windows 名称规则](../reviews/2026-11-24/platform-sources.md)

**修复与验收。** 以工作区目录句柄为起点逐级取得实际使用的目录和文件，把已检查的句柄交给下载与上传。对允许跟随的链接检查最终位置和保留组件，复用一致的 Windows 名称规则。验收应覆盖并行目录轮换、指向工作区内保留目录的别名和 Windows 设备名，且工作区外既有文件保持原内容。

### F05 · 高 · Files 流撤权后，已启动的递归修改仍继续

**位置与证据（静态）。** `crates/ma2a-runtime/src/stream_host/inbound.rs:159–169` 的优先分支会在取消时结束网络流。`crates/ma2a-services/src/files.rs:56–90` 已把内容无关的操作交给持有并发许可的阻塞闭包。`files/ops.rs:145–159` 及 `files/dir_unix.rs:124`、`files/dir_windows.rs:146` 的递归删除没有接收取消状态。丢弃外层等待不会停止已运行的闭包。

**触发与影响。** 操作者在长递归操作中撤销暴露。网络流和运行记录可已结束，工作线程仍可继续删除尚未处理的条目。16 个阻塞任务的并发上限只约束任务数量。协议文档已说明普通超时可能留下阻塞任务，但持续鉴权一节同时承诺撤权会停止操作。

**修复与验收。** 把取消状态传入可分段执行的文件操作，在开始下一项修改前检查，并记录已发生的部分效果。用受控同步点暂停遍历、撤权、恢复，确认剩余条目不会被修改，且运行记录能区分取消与成功。

### F06 · 高 · 远端下载核对了摘要，未核对接受时声明的完整长度

**位置与证据（静态）。** `crates/ma2a-services/src/client/transfer.rs:37–66` 仅比较终帧的本次字节数与实收字节数、终帧的摘要与本地摘要。函数没有接收接受阶段的身份或起始偏移，也没有校验终帧的 `size`。`crates/ma2a-app/src/commands/remote/agent.rs:233–247` 和 Runtime 工具的 `download` 均在此后发布文件。普通 `file get` 也共用该函数。与此相对，本地 Agent 制品下载逐块检查上界，并在发布前核对长度。

**触发与影响。** 对端版本异常或返回前后不一致时，接收端可以报告成功并发布与接受长度不符的文件，或在发现问题前写入超过声明长度的数据。摘要只验证它实际收到的字节。当前仓库发送端通常会产生一致的三项数据。

**修复与验收。** 将接受的身份、续传偏移交给内容接收器。写入前校验剩余额度，发布前同时校验完整大小、本次传输量和全文件摘要。验收覆盖空文件、非零续传偏移、短读、超长以及终帧总长不一致，失败时不得发布。

### F07 · 中 · 浏览器能在服务端会话仍有效时显示已登出

**位置与证据（静态）。** `web/src/app.tsx:107–128` 对包装后的 401 和 403 都清空页面状态并跳回登录页。`crates/ma2a-runtime/src/web.rs:92–109,135–148` 也会因 CSRF 不匹配或内部认证错误返回 403，只在成功删除会话后清 Cookie。`web/src/api/web-auth.ts` 将 CSRF 保存在当前页面的会话对象中。

**触发与影响。** 一个标签页重新登录、替换共享 Cookie，另一标签页仍持有旧 CSRF。旧标签页点击退出会看到登录页，而当前浏览器会话仍在服务端有效，重新载入可以恢复登录态。临时 Store 错误也可能被误报为已退出。

**修复与验收。** 区分凭据失效、CSRF 冲突和内部失败。CSRF 已轮换时，用浏览器当前会话重试一次或明确提示未完成。双标签页验收须检查刷新后的认证状态和服务端记录，而不仅是当前页面路由。

### F08 · 中 · 流关闭时限没有覆盖首帧、监督器首答和公共终帧

**位置与证据（静态）。** Process 与 Agent 的 `finish` 已给 outcome 写入 5 秒宽限。`crates/ma2a-runtime/src/processes/attach.rs:62–84,105–128` 在进入可取消的转发循环前等待监督器首答并写 accept。`processes/stdin.rs` 也先调用 `open_live`。`crates/ma2a-services/src/exec.rs` 和 `shell.rs` 在创建子进程后直接等 accept 写完；`crates/ma2a-services/src/reply.rs:19–42` 的公共终帧及关闭没有时限。

**触发与影响。** 监督器已建立连接但不再回答，或流控使首帧或终帧无法写完时，撤权后的任务和许可可超出所述宽限。Attach 已占用 reader slot；Exec/Shell 若停在首帧，清理子进程的逻辑也要等待。该问题无需否定在主转发循环中已生效的取消检查。

**修复与验收。** 给握手、首帧和关闭操作相应的截止时间与取消分支，退出时先释放本地进程、句柄和租约。用测试传输和监督器分别暂停三个阶段，验证撤权后资源有界释放，并保留已完整发送的终帧。

### F09 · 中 · Windows 在系统服务账户下无法满足 Store 的 ACL 谓词

**位置与证据（静态，Windows）。** `crates/ma2a-store/src/permissions.rs:174–199` 要求恰好两条显式规则、每个 SID 唯一、且仅允许当前 SID 或系统 SID。当两者相同，单条规则不够，两条相同规则又因重复被拒。邀请文件和 relay 密钥的独立谓词虽可接受两条同主体规则，也仍硬性要求两条，见 `space_windows_policy.rs:18–49` 和 `relay_tls_windows.rs:145–180`。

**触发与影响。** 使用 LocalSystem 运行时，合法的私有对象也可能导致初始化或读取失败。Store 的矛盾与系统是否合并相同 ACE 无关。普通用户、UAC 提升后的管理员组所有者不触发这个特定矛盾。

**修复与验收。** 根据预期主体去重后验证有效权限，统一 Store、邀请和 relay 规则。原生测试应实际用系统服务身份创建、重开目录及文件，并验证额外主体仍被拒绝。

### F10 · 中 · Windows 只凭 RID 推断 LA 属于当前账户

**位置与证据（静态，Windows）。** `crates/ma2a-services/src/sddl.rs:11–23,43–49` 把所有符合账户 SID 前缀且以 RID 500 结尾的 SID 都映射为 LA。LS、NS 映射的是固定完整 SID。Microsoft 的《SID Strings》和《Security identifiers》把 LA 定义为本地管理员，并说明本机和域各自有 RID 500 的账户。原测试只给一个 RID 500 示例。[Windows SID 资料](../reviews/2026-11-24/platform-sources.md)

**触发与影响。** 域和本机账户前缀不同时，该谓词没有足够信息断言 LA 与当前进程是同一主体。因此不能用它证明第八轮 F03 所述的账户相等性。项目本就信任管理员组；本项没有把别名误判推定为普通账户提权，也未实测该部署上系统实际输出的 SDDL 拼写。

**修复与验收。** 直接比较句柄安全描述符中的二进制 SID，或由系统解析别名后比较完整 SID。原生测试覆盖同 RID 不同账户域、本机内置账户、LS、NS 和管理员组所有者。

### F11 · 中 · Windows 邀请输出未固定父目录，单文件名还会在提交后失败

**位置与证据（静态，Windows 回归）。** `crates/ma2a-runtime/src/ipc/server/execute/invite_file.rs:32–40` 要求父路径非空，因此单一相对文件名被拒。`19a42fb` 的 Windows 实现直接对输出路径做独占创建。当前 Unix 仍接受这种文件名。Web 表单只要求非空；`execute/space.rs:37–60` 先提交邀请，再尝试写文件。第 44–62 行的创建、改名和失败清理也都重新使用原始父路径；创建文件时提供的保护性 ACL 和不覆盖改名只能约束各自那一次操作。

**触发与影响。** Web 或本地 API 传入普通文件名，Windows 已生成邀请、推进 revision，却不能交付票据。CLI 自选状态目录内路径，通常不触发单文件名问题。若上级目录别名在发布时轮换，改名或清理会查找另一个目录，原目录可残留受 ACL 保护的票据和暂存目录。“失败保证清理”在这里也不成立。

**修复与验收。** 按既有语义把空父路径当作当前目录，在创建邀请前解析并固定目的地，发布和失败清理均使用同一已验证目录。若产品要求改用绝对路径，应同步更改协议、界面并在提交前拒绝。用单文件名、带父目录的相对路径和同名既有文件检查跨平台结果以及 revision；再检查父目录别名轮换后原暂存物的归属和清理。

### F12 · 中 · 检查 partial 失败会被当作确认零字节

**位置与证据（静态）。** `crates/ma2a-services/src/client/resume.rs:138–145` 忽略 flush 错误，也把读取元数据失败折叠为 false。`crates/ma2a-app/src/commands/remote/transfer.rs:36–44,97–111` 据此决定删除 partial 与 sidecar。保留分支再次 flush 也丢弃其错误。

**触发与影响。** 断流时，若本地文件状态查询恰好失败，已有可续传的前缀可能被当成空文件删除；若异步写入报错，也可能只向操作者报告网络错误。正常 flush 后再取长度的顺序已修复。

**修复与验收。** 让检查返回可区分 I/O 错误的结果，仅在确知零长度或内容无效时丢弃。测试写入延迟、flush 出错和元数据读取失败，确认已有进度可恢复，且本地故障可见。

### F13 · 低 · 带顶层目录的归档允许夹带空子目录

**位置与证据（实测）。** `scripts/validate-release.sh:38–54` 在比对清单前排除了所有目录记录。相同六个条目下，平铺包和单根包均通过；添加一个空子目录后平铺包被拒，单根包仍通过。四种布局的退出码依次为 0、0、1、0。

**触发与影响。** 多余目录进入带统一前缀的归档时，“无杂项目录”的校验结果因包装形式而异。该实测只检验条目清单，没有执行 Windows 二进制。

**修复与验收。** 只允许可选的顶层目录记录，逐项校验其余相对名称和类型。对两种 ZIP 布局及共用前缀处理的 tar 归档使用同一组验收条件。

### F14 · 低 · SBOM 校验器未解析 documentDescribes 的引用

**位置与证据（实测）。** `scripts/generate-sbom.sh` 为本次 Linux 数据生成了 316 个包及一个实际的应用包引用。`scripts/validate-release-metadata.sh:13–26` 只检查该数组长度为一，并另行检查包列表里存在应用。在真实生成的 SBOM 副本中将描述对象改为一个不存在的包 ID，生产校验器仍退出 0。归档摘要未变。

**触发与影响。** 生成器重构或下游编辑造成悬空引用时，发布门禁仍声称验证成功。现有生成产物的该引用正确，第八轮的归档内容绑定也仍成立。

**修复与验收。** 要求描述对象唯一解析到预期应用包的 SPDXID，并检查包标识唯一。现有产物应通过，悬空、重复以及指向其他现存包的引用应失败。

### F15 · 低 · Files 列表仍返回协议无法寻址的 Unix 文件名

**位置与证据（Linux 实测）。** `crates/ma2a-services/src/files/ops.rs:55–87` 的过滤覆盖 UTF-8、字节长度和控制字符；`crates/ma2a-core/src/service/names.rs:123–132` 的组件语法还排除冒号、反斜杠。通过真实的 Files host 和编解码器列目录，得到 `listed=phase:one.txt addressable=false`。

**触发与影响。** 含这些普通 Unix 文件名的导出目录可以列页，但客户端无法用同一个名称构造 stat 或下载请求。第八轮 F09 的长 UTF-8 名称和第九轮的控制字符修复仍有效。

**修复与验收。** 复用协议组件验证器过滤候选项，再排序、分页。每个承诺可访问的返回名称都应能构造后续请求，跳过不可表示的项也不能跳过后面的有效项。

### F16 · 低 · unsafe 门禁把合法 Unicode 标识符的一部分读成关键字

**位置与证据（实测）。** `xtask/src/unsafe_scan.rs:15–29` 按 ASCII 边界分词，预处理又将普通 UTF-8 字节分别转换为字符。探针只有一个含非 ASCII 前缀的完整标识符，Rust 1.91 在禁止 unsafe 的设置下正常编译，生产扫描函数却回报 `(true, false)`。仓库门禁按出现关键字的文件集合做精确比较。

**触发与影响。** 正常 Rust 源文件可以使 CI 误报新的 unsafe 边界。本次验证的是误报，没有执行绕开门禁的代码。

**修复与验收。** 使用与当前 edition 一致的 Rust token 和标识符边界，保留现有注释、字面量、原始标识符和 lint 测试。新增合法 Unicode 标识符用例后，扫描结果应与编译器词法一致。

### F17 · 低 · LOC 门禁把行尾开始的块注释算成代码

**位置与证据（实测）。** `xtask/src/loc.rs:46–88` 在发现行首是代码时直接返回，未记录该行后半段开始的跨行注释。生产计数函数对一段有 3 行代码的 Rust 函数返回 5；额外两行分别只有注释内容和注释结束符。

**触发与影响。** 接近 250 行上限的正常文件可因注释换行而误触门禁。仓库当前版本的 `check-loc` 通过。

**修复与验收。** 判定本行是否贡献 LOC 后继续扫描到行末，保留跨行词法状态。覆盖行尾起始、嵌套注释、结束符后的同一行代码和字符串中的注释定界符。

## 第九轮修复逐项复核

第九轮仅留修复摘要，其中 M01/M02、M03/M04、M05/M06 和 M07/L01–L03 分组记录。下表保留这些分组，按其中的实际改动核对，避免给已缺失的原报告补造编号含义。

| 原项与提交 | 本轮判断及证据 |
| --- | --- |
| B01 · `1167847` | 成立。锁文件解析到 `url 2.5.9`、`form_urlencoded 1.2.3`，实际执行的依赖检查四类均通过。 |
| B02 · `3fc2021`、`c4f72a2` | 部分成立。Windows 冷启动的常见单级路径改为创建时提供 DACL。邀请创建用 `CreateFileW(CREATE_NEW)` 指定受保护 DACL，同卷无覆盖改名使用标志为零的 `MoveFileExW`，两个 FFI 入口拒绝内嵌 NUL。CLI 在读私有邀请失败时也尝试删除临时文件，写入移到阻塞池。测试夹具、冒烟脚本的 Windows 目录准备、二进制引用、回车和请求体重定向均已更新。冷启动、服务账户和邀请路径的剩余条件见 F03、F09、F11。最终提交仅收窄 Linux 测试的导入作用域。 |
| B03 · `e354d0f` | 平铺 Windows ZIP、已有单前缀布局、缺文件和混合布局的基本规则成立；带前缀的额外目录仍被接受，见 F13。 |
| H01 · `dd8232d` | 本地 Agent 先成功读取首块，远端制品和工具先收到接受帧，之后才打开 partial。写内容前调用 `begin`，本地制品累加偏移做溢出和上界检查。非续传发布错误走 `finish_or_discard`；续传文件在判断长度前发起 flush。未知制品或远端拒绝不再因打开 partial 而清掉同目标的旧进度。成功接受之后，非续传打开仍按从头下载的语义处理。剩余的长度一致性和 I/O 错误处置见 F06、F12。 |
| H02 · `3fc2021` | 源码成立。Windows Files 根目录先打开句柄、验证目录类型，再保存该句柄查询到的最终路径。Windows GNU 全目标 Clippy 已通过；本机没有实际轮换 Windows junction。 |
| H03 · `3fc2021`、`dd8232d` | 部分成立。受保护对象明确授予当前用户 SID 和系统 SID；邀请与 relay 检查允许受信所有者包含管理员组，同时仍限制 DACL 的主体。文件身份优先读取 128 位标识，最终路径缓冲区增长有上限，空 SDDL 文本不会直接解引用。LS、NS 的固定映射与平台定义一致。LocalSystem 的规则计数和 LA 的账户域假设分别见 F09、F10。 |
| H04 · `47134da` | 成立。`control_sync/pages.rs:58–90` 在连续 manifest 前缀截断时立即返回，只有完整前缀才附其他记录。完整页上的本地 relay 公告也检查当前提供者权限。实际通过的签名页回归用例为 `truncated_push_page_omits_records_until_full_prefix_fits`；第八轮留下的这个待确认项可以关闭。 |
| H05 · `47134da` | 对“票据过期”和“成员已被移除且当前不在 Space”两种原条件成立。`enrollment.rs:177–201` 在同一写事务中检查有效期、候选及请求身份、当前成员关系，再返回原来的响应链。有效期内当前成员的准确重试仍返回原提交链，符合协议的幂等语义。没有把返回历史响应本身认定为新增授权。 |
| H06 · `6b3858d` | 部分成立。登记后绑定 root 并立即检查取消，Files、Transfer 和 Agent artifact 分派优先处理已就绪的取消。完整终帧与中断写入现在可区分，Process/Agent 的最终 outcome 有写入宽限。长操作的本地副作用、首帧和关闭等待见 F05、F08。 |
| H07 · `ac71653` | 成立。`web/auth.rs` 在打开服务时创建 Repository，后续 setup、登录、touch、登出和批量撤销复用同一连接。密码计算在互斥锁外；提交会话仍通过 `create_session_if_current` 核对凭据世代。 |
| H08 · `ac71653` | 核心改动成立。私有 IPC 只给带请求 ID 的命令加重放锁。Web 路由撤掉 redeem，HTTP 和 Web→IPC 截止时间均为 120 秒，撤销全部会话仅在非错误返回时过期 Cookie。监听器明确选 HTTP/1，限制 64 个并发连接、10 秒收请求头，临时 accept 失败退避并回收任务。Web 查询 30 秒、变更 120 秒，变更错误可解析可空 remediation。Space 与 relay 确实按表单显示消息，但同一面板仍共用 busy 守卫；摘要所称“每表单独立 busy”并不准确。登出状态另见 F07。 |
| H09 · `12e2d5a` | 非 Windows 的 crate 级禁止规则、现有九个受审计边界的文件集合检查、原生 CI 矩阵的全目标 Clippy 均在。源码词法检查原回归集通过；本轮 Windows GNU 交叉 Clippy 通过。合法标识符仍能造成误报，见 F16。 |
| M01/M02 · `dd8232d` | 列表增加控制字符过滤，下一页游标取扫描过的最后一个名字，能跨过在 stat 前消失的条目。两平台父目录消失的判断、Windows 探测句柄的共享方式均已调整。工作区工具补充名称检查并在接受后复核目的地。上传同名竞争开始保留日志。列表、工作区和上传日志的剩余边界分别见 F15、F04、F02。 |
| M03/M04 · `47134da`、`6b3858d` | 多 Space 地址合并在加入去重集合前检查数量上限，同时继续比较后续记录的用户数据。Process 日志数据写入可取消，stdin 转发先释放监督器通道再写终帧；更短的 terminate 会提前已有截止时间。Agent watch 的事件写和 flush 有界，Hosted SDK 的通知读取会响应接收端关闭，Agent SDK 的超限提问会返回并上报失败。流全生命周期仍见 F08。 |
| M05/M06 · `ac71653`、`331368b` | 多个 Cookie 头会一起检查，空或重复的会话 Cookie 被拒。快照曾成功后可持续退避重试，清空详情不再把尚未返回的 worker 计数清零。登出已能拆开包装错误，但 403 仍有歧义，见 F07。CLI 在子命令匹配前处理全局状态路径并锚定命令起点，现有远端命令透传回归通过；状态显示分别统计两类 relay，A2A ready 行读取实际认领路径。 |
| M07/L01–L03 · `12e2d5a`、`e354d0f`、`3778d4f` | CSS 引用测试覆盖引号和无引号形式；真实 JS 的问题见 F01。SBOM 时间归一到 UTC、声明应用包、修正许可证斜杠和下载位置，摘要绑定保持。校验盲点见 F14。发行/E2E 脚本有回车和哈希工具兼容处理；本轮只运行 Linux 分支与布局测试。LOC 扫描、构建输入、React 列表 key 及 CLI/协议/快速入门改动在树中，LOC 边界见 F17。 |

## 第八轮 F01–F15 回归核对

| 第八轮 | 对原问题的判断 | 当前证据与边界 |
| --- | --- | --- |
| F01 备份路径 | 保持 | `ma2a-store/src/backup.rs:30,166` 继续使用校验返回的规范目标。Linux 备份套件覆盖暂存、发布和恢复。 |
| F02 Windows 下载父目录 | 保持原修复 | Windows anchor 取最终路径、发布前核对父目录及 partial 身份、发布后核对目标身份。第九轮增加“已消失”结果和共享标志。Linux 轮换测试与 Windows 交叉构建的证据强度不同。 |
| F03 Windows 下载目录权限 | 部分回归 | 创建时的受保护 DACL 保留，明确用户 SID 也改善 UAC。新增 LA 规则未证明完整账户相等，见本轮 F10。 |
| F04 撤权早于绑定根目录 | 保持 | `stream_host/reauthorize.rs:167–174` 将 Pending 保持为未决，同 revision 能重新检查。定向用例通过。本轮 F05、F08 出现在取消传递之后。 |
| F05 Agent 制品按名发布 | 保持原修复 | CLI 本地和远端均用 Partial 和无覆盖发布。远端协议的完整长度校验是本轮 F06。 |
| F06 删除操作者的标记 | 保持 | `client/resume/helpers.rs:127–188` 检查标记完整内容，预建目录和操作者文件的 Linux 回归用例在测试清单内。 |
| F07 离场回执无界 | 保持 | 每个发送者和移除世代的绑定上限仍为 8，达到上限走请求拒绝，已有索引与历史前缀语义保留。 |
| F08 A2A 锁与票据目录分离 | 保持并补全展示 | `a2a/token_file.rs:94–125` 的 Claim 保存认领位置；`a2a.rs:66–79` 写入、退出清理与 ready 行共用这个位置。 |
| F09 Windows 长文件名导致页无法解码 | 保持 | 长度在取页前按 UTF-8 字节检查；控制字符也被过滤。本轮 F15 是列表可解码后却不能用该名称发起下一次操作。 |
| F10 上传清扫归属说明 | 出现新回归 | 原文档披露了中断现场及路径复用，但第九轮新增了无中断、普通重启也能误删的条件，见本轮 F02。 |
| F11 并行 E2E 的 JUnit 归档 | 原条件保持 | 脚本从 Cargo metadata 取目标目录，并在该处持锁完成删除旧报告、运行和复制。本轮静态核对互斥范围；没有开展双进程并发归档实验。 |
| F12 超大运行次数先清证据 | 保持 | 1–9999 和前导零检查仍位于接触输出目录之前；仓库自带的次数及目录复用测试通过。 |
| F13 SBOM 与归档绑定 | 保持 | 校验器仍重新计算归档 SHA-256，本次包含“同名重建归档”负例的发行测试通过。见本轮 F14 的内部引用完整性问题。 |
| F14 Agent 工具长暂存名 | 保持 | 下载辅助名仍基于哈希，锁名复用单个目标 basename，没有拼接会使 255 字节组件超限的后缀。 |
| F15 A2A ready 写失败遗留凭据 | 保持 | `Published` 在写 ready 行之前已经持有清理责任，`writeln!` 出错会进入析构。第九轮只调整了 ready 文本中路径的来源。 |

## 独立审查的覆盖边界

从发送端、接收端和持久化调用者反向检查了 Core 的路径与 Transfer 编解码、Store 的事务与恢复、Net 的地址合并和 TLS 私钥检查、Runtime 的入网与控制同步、IPC/Web 准入、Files/Transfer/Process/Agent 的授权与完成路径。应用层核对了本地和远端 CLI、A2A 票据、两个 SDK 的任务结束与通道关闭，以及 Web 的快照、详情、表单和会话。发布部分用同一次构建贯通了归档、许可证、SBOM、嵌入前端和真实 HTTP 冒烟。独立检查发现了跨模块才可见的 F01、F02、F05–F08 和 F15。其余代码按调用边界抽查，没有逐行审计每个未改文件。

本机为 Linux。Windows GNU Clippy 可检查条件编译和类型；NTFS/ReFS、PowerShell、UAC、系统服务身份、Windows MSVC 运行和 macOS 原生行为没有在本轮执行。没有签发生产制品，也没有将来自用户的 CI 状态计作本机实测。

## 实际执行的验证命令与结果

以下命令从仓库根目录运行，示例使用 Bash。探针与验证日志位于 `.omo/reviews/2026-11-24/`。本轮结论共 **17 项，高 6、中 6、低 5**。F01、F13–F17 有对应的结果差异实测，F02–F12 的特定异常路径为静态判断；相关正常路径的测试通过不能替代这些判断的验收。

### 1. 基线与已跟踪文件

```bash
git rev-parse --short HEAD
git rev-parse --short origin/master
git rev-list --count 19a42fb..c4f72a2
git log --oneline 19a42fb..c4f72a2
git diff --stat 19a42fb..c4f72a2
git diff --check 19a42fb..c4f72a2
git diff --quiet
git diff --cached --quiet
git status --short --branch
```

两个引用均为 `c4f72a2`，计数 11。范围内空白检查、已跟踪工作树和暂存区检查均退出 0。审查前已有未跟踪的历史探针目录。本轮新建报告和本日探针，没有提交。

### 2. Rust 全仓测试、Windows 交叉编译和门禁

```bash
review=.omo/reviews/2026-11-24
export CARGO_TARGET_DIR="$review/target"
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0
cargo nextest run --locked --workspace --all-features -j 8 > "$review/rust.log" 2>&1
cargo nextest list --locked --workspace --all-features --message-format json > "$review/test-list.json"
python3 "$review/remaining-tests.py" > "$review/remaining.log" 2>&1
CFLAGS_x86_64_pc_windows_gnu=-D_WIN32_WINNT=0x0601 \
  cargo clippy --locked --target x86_64-pc-windows-gnu --workspace --all-targets --all-features -- -D warnings
cargo run --locked -p xtask -- check-loc
cargo fmt --all -- --check
cargo deny check
```

Rust 1.91.0，nextest 0.9.143。首轮 850/1007 已运行，849 通过、1 个在 120 秒超时，退出 100。补跑依据首轮 PASS 名单选择其余 158 个，使用项目的 4 个并行测试槽，将单例超时上限延至 360 秒，158/158 通过。`replay_entries_survive_reopen_and_evict_oldest_by_count` 补跑 90.550 秒通过。以二进制名和用例名对两次 PASS 取并集，与完整清单比较，**1007 个不同用例均通过，缺项 0，额外项 0**。这是两次执行的合并结果，首轮超时仍计入记录。

其中备份 6、离场回执 3、下载目录 8、sidecar 8、客户端 Transfer 7、Files host 7、满员与入网重试 2、CLI 邀请安全 4 项通过。其余包含控制同步签名页、重新鉴权、IPC、Web、Agent/Hosted SDK、Process、A2A 及 xtask。Windows GNU 全目标 Clippy、LOC、rustfmt 均退出 0。依赖检查的 advisories、bans、licenses、sources 四类均通过。最初尝试的 `cargo deny check --disable-fetch` 在参数解析阶段退出 2，上述正常命令重跑后才取得依赖检查结果。

### 3. Web 与固定工具版本的前端构建

```bash
(cd web && bun run test)
(cd web && bun run tsc --noEmit && bun run biome check .)
(cd web && npm exec --yes --package=bun@1.3.5 -- bun --version)
(cd web && npm exec --yes --package=bun@1.3.5 -- bun run build --outDir ../.omo/reviews/2026-11-24/web-dist)
cmp web/dist/assets/index-CkJE4rn4.js "$review/web-dist/assets/index-CkJE4rn4.js"
scripts/extract-runtime-references.pl js "$review/web-dist/assets/index-CkJE4rn4.js" | rg -F target_endpoint_id
```

24 个 Vitest 文件、153 项测试全部通过。类型检查通过，Biome 检查 118 个文件通过。初次构建时环境默认 Bun 为 1.4.0，另以项目指定的 1.3.5 运行构建，比对入口 JS 退出 0。最后一条命令实际输出一行以 `asset` 开头的 JS 表达式片段，对应 F01。没有执行浏览器双标签页或 Windows 原生 UI 自动化。

### 4. 发布正向冒烟、失败用例及证据目录

```bash
scripts/test-runtime-references.sh
scripts/test-e2e-output-reuse.sh
bash "$review/check-release.sh"
python3 "$review/check-zip.py"
```

- 仓库的资源扫描回归和 E2E 输出复用测试均退出 0。后者使用仓库自带的模拟 Cargo，只检验参数及目录处理。应用 E2E 用例包含在上面的 Rust 测试清单中。
- 发布探针使用当前提交新编译的 Linux 调试二进制、README、quickstart、NOTICE 和两份许可证组装本地归档，计算 SHA-256 并调用生产脚本生成许可证清单和 SBOM。归档正向校验、316 包 SBOM 正向校验均通过。真实 HTTP 冒烟在资源检查退出 1。探针继续运行后，仓库自带的 `test-release-failures.sh` 仍报告成功，说明现有负例并不能证明正常包可发布。探针最终保留正向冒烟的退出码 1。
- F14 使用真实 SBOM 的副本，仅将描述对象改成一个不存在的 ID；元数据校验意外退出 0。独立 ZIP 探针的四个退出码为 `0 0 1 0`。ZIP 中可执行文件条目使用**仅供布局测试的字节夹具**，其余文档来自仓库。Windows 可执行性由此无法验证。未把该夹具、调试归档或 SBOM 当作正式发行产物。

### 5. 独立 Files 与词法探针

```bash
bash "$review/check-list.sh"
rustc --edition=2024 -F unsafe-code "$review/check-policy.rs" -o "$review/scan-probe"
"$review/scan-probe"
rustc --edition=2024 -A dead_code "$review/check-loc.rs" -o "$review/loc-probe"
"$review/loc-probe"
```

Files 探针链接本轮构建的库，在本日探针目录的临时子目录中创建普通文件，通过内存传输调用真实的列表服务，输出 `listed=phase:one.txt addressable=false`。两项词法探针直接调用生产扫描函数，输入均为**仅供测试的合法 Rust 片段**。前者输出 `keyword_lint=(true, false)`，后者输出 `pure_lines=5, expected=3`。全部退出 0，差异在输出值中。探针源码和测试记录已保留，重建 Rust 库后即可重跑。
