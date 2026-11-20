# MA2A 第八轮代码审查

基线为 `master` 与本地 `origin/master` 共同指向的 `4af6c24`。比较范围 `210ac51..4af6c24` 共 20 个提交、60 个文件。先按第七轮报告及处理记录逐项追踪到实现和测试，再从发布、持久化、协议编解码与实际调用者反向检查。下文的 F 编号均属本轮。“静态”表示结论由当前源码的控制流、数据流或协议约束得出。验收栏是建议加入的回归条件，实际运行结果列在文末。

## 已确认的问题

### F01 · high · Unix 备份检查的目录与 SQLite 后续打开的目录可能不同

**位置与证据（静态）。** `crates/ma2a-store/src/backup.rs:165–181` 规范化目标的父目录、检查规范路径各级祖先的属主和权限，返回 `()`。第 31–42、89–92、107–111 行仍用调用者原来的路径创建暂存目录、打开 SQLite 并发布。对文件句柄的身份核对发生在发布阶段。它无法证明当初创建暂存目录时使用了检查通过的父目录。

**触发与影响。** 操作者通过目录别名备份，别名位于可被其他账户改名的目录下，最初指向受信目录。若别名在权限检查与创建暂存目录之间改指，随后全程可以在另一个目录完成，包括完整性检查与发布。原本应被权限检查拒绝的目标因此也可能得到成功结果。运维时轮换别名同样可以造成备份写错位置。此结论无需假定 SQLite 本身产生坏数据。

**修复与验收。** 让检查返回已验证的父目录，SQLite 暂存、发布、清理全程使用同一对象；可在受信规范目录内操作，或固定目录句柄并验证最终名称。验收应在目录检查后安排一次别名轮换，确认只可能在原先受信的目录生成可恢复备份，或明确报错；另一目录保持原样。

### F02 · high · Windows 下载固定了目录句柄，发布时仍会重新解析可变的目录别名

**位置与证据（静态，Windows）。** `crates/ma2a-services/src/client/resume/anchor_windows.rs:47–68` 跟随目标路径打开目录并持有无删除共享的句柄，同时保存原路径拼写；第 91、118–129 行按保存的路径重新打开文件或改名。`crates/ma2a-services/src/client/resume/end.rs:38–65` 的发布前后身份比对仅在 Unix 执行，Windows 在按名发布前关闭已校验文件。

**触发与影响。** 下载路径经过可轮换的父级 junction 或目录符号链接。替换该别名无需移动被固定的目标目录。若别名新指向的目录内已有同目标名的暂停下载，最终发布和清理可作用于另一份 partial 和 sidecar，使成功下载的内容与本次核对的摘要不符；缺少同名状态时通常表现为失败。普通目录直接改名的回归用例没有覆盖此条件。

**修复与验收。** 以所持句柄定位子项并发布，或验证整条名称解析链和发布文件的身份，发现变化时保留双方进度。Windows 原生用例应覆盖父级别名轮换，两份下载使用不同摘要。成功产物必须匹配本次摘要，另一份的续传位置与内容应保持完整。

### F03 · high · Windows 共享目标目录中的下载暂存文件允许其他账户改写已计入摘要的数据

**位置与证据（静态，Windows）。** `crates/ma2a-services/src/client/resume/helpers.rs:45–63` 只在 Unix 验证辅助目录私有性。`crates/ma2a-services/src/client/resume/anchor_windows.rs:71–91` 使新建目录继承目标父目录的 ACL，普通文件的共享模式含 `FILE_SHARE_WRITE`。`crates/ma2a-services/src/client/transfer.rs:52–55` 对流中刚写入的字节更新哈希；`crates/ma2a-services/src/client/resume/end.rs:23–36` 发布前执行 flush 和 sync，没有重读文件内容。

**触发与影响。** 目标位于向其他账户继承授予写权限的共享目录。同一 partial 的已写部分在下载尚未结束时被另一个有权写入的进程修改。传输的字节流仍能通过摘要校验，交付文件却包含被修改的旧区段。这个条件不需要替换目录或改变文件身份，与 F02 的修复和验收不同。

**修复与验收。** 创建辅助目录时直接赋予私有 DACL，复用时验证属主与实际访问权限；活动 partial 应限制外部写共享。Windows 原生测试在可共享的父目录中检查新建和复用两种情形，验证其他普通账户无法改写活动文件，完成后的内容与传输摘要一致。

### F04 · high · 服务授权尚待绑定时，复验可以提前确认新 revision

**位置与证据（静态）。** `crates/ma2a-runtime/src/stream_host/inbound.rs:68–95` 先登记 Pending，再异步取得授权根并绑定。`crates/ma2a-runtime/src/stream_host/reauthorize.rs:36–39` 允许 Pending 匹配任意根；第 102–115、147–163、174–182 行复验快照中的根，全部有结论即记住 revision。第 121–124 行绑定旧授权结果时只更新表项，不要求再次复验。

**触发与影响。** 一次有根的长操作已从 Store 取得旧目录的许可，但入站任务尚未消费返回值。操作者撤下原暴露、以同名暴露另一目录。复验任务先在新 revision 取到 Pending 快照，新根符合 Pending，随后入站任务按旧根开始操作。如果状态不再变化，该连接可持续访问已经撤下的原目录，超过约定的一次复验周期。

**修复与验收。** 把尚未绑定根的检查记为未决，或令绑定带上授权版本并强制一次新复验。用可控的 Store 返回顺序交错两个任务，检查旧根操作被取消、同根操作继续、一次 Store 暂不可用后仍会重试。

### F05 · high · 本地与远端 Agent 制品下载仍可能发布另一份文件

**位置与证据（静态，Unix）。** `crates/ma2a-app/src/commands/agent_artifact.rs:55–73` 从最终名称派生固定兄弟文件，在第 86–106 行对收到的数据算摘要、同步句柄，再按路径硬链接及删除。远端入口 `crates/ma2a-app/src/commands/remote/agent.rs:231–251` 使用相同命名策略，并在发布前关闭句柄。两处均绕开这次修复的通用下载目录锚定逻辑。

**触发与影响。** 两个目录各有下载到同名文件的任务，其中一个目标目录在下载期间被轮换。第一个任务可将接替目录里第二个任务仍在写入的 partial 硬链接为成功产物，再删除第二个任务的暂存名。此时打印的成功摘要属于第一个任务，磁盘产物可能继续被第二个任务改写；第二个任务随后也可能因路径消失而失败。整个场景只需正常文件操作。

**修复与验收。** 本地 IPC 和远端制品入口都复用按所持目录和文件身份发布的机制，保留现有“目标已存在则拒绝”的行为。分别对两种入口执行同名并行下载与目录轮换，验证成功回执、产物摘要一致，且未清理其他任务的 partial。

### F06 · medium · 清理下载进度会删除操作者已有的标记同名文件

**位置与证据（静态）。** `crates/ma2a-services/src/client/resume/place.rs:22–26` 将标记命名为 `created`。`crates/ma2a-services/src/client/resume/helpers.rs:79–88` 只在本次新建辅助目录时写标记，但第 132–153 行仅凭目录中有同名普通文件就删除它；目录仍非空时会重新创建空标记。标记检查前还会尝试移除空的 `locks` 子目录。

**触发与影响。** 一个操作者预先创建、权限合格的 `.ma2a-downloads` 目录，已有名为 `created` 的非空普通文件。一次普通下载成功或丢弃后，该文件被删除；有其他目录项时，它可能以零字节重新出现。上轮针对预建空目录的测试仍会通过。

**修复与验收。** 使用独占创建、有固定版本和内容的所有权标记，遇到未知内容保留；独立记录锁目录是否由下载创建，或保守地保留它。在预建目录内分别放置有内容的 `created`、预建的空 `locks` 和普通文件，验证下载完成及丢弃都保留既有对象和内容，自动创建的辅助目录仍可清理。

### F07 · medium · 已离场成员的新请求 ID 可以持续增加持久回执

**位置与证据（静态）。** `crates/ma2a-runtime/src/store/membership.rs:47–73` 对已记录的请求重放，对已离场端点的每个未见 ID 则绑定同一个历史移除代。`crates/ma2a-store/src/departure_receipts.rs:50–64`、`:117–128` 为每个绑定新增一行，且在立即写事务内加载并验证链。第 134–140 行在遍历时每代克隆当前前缀；`crates/ma2a-core/src/space_chain.rs:74–81` 的前缀拥有历史向量。

**触发与影响。** 合法离场身份针对同一历史移除代提交不同请求 ID 时，即使此后没有任何成员变化，数据库仍随 ID 数量增长。较长的历史还会放大每次持有 SQLite 写事务的工作量。Space 代数上限约束历史长度，不能约束指向同一代的回执数。固定 ID 的普通重试仍正确且幂等。

**修复与验收。** 为一个端点、一个移除代可接受的新增绑定设定上限，满时明确拒绝新 ID 并保留所有已答复绑定；长期方案可让请求指明入会代际。计算历史边界时只在目标代构造所需前缀。验收同时检查回执行数有界、原 ID 在重新入会后始终重放旧答复、当前成员资格不受影响。

### F08 · medium · A2A 的凭据锁和实际发布文件可分属不同目录

**位置与证据（静态）。** `crates/ma2a-app/src/a2a/token_file.rs:33–55` 仅用规范路径检查保留名，返回的 parent 保留别名。第 75–87 行在当时的父目录取得锁；第 124–140 行发布时重新解析路径。`crates/ma2a-app/src/a2a.rs:67–75` 在两步之间还异步绑定监听器，退出时第 98 行再次按原路径清理。

**触发与影响。** 部署以目录别名指定 `--token-file`，在取得锁后轮换别名。适配器可继续持有旧目录的锁，却向新目录发布凭据；另一个使用新目录的正常实例也能取得自己的锁并向同一文件发布。先启动实例失去可用的凭据发现文件，运维工具还可能拿到另一个监听器的令牌。两个静止 state 目录共用一个固定凭据路径的原问题已修。

**修复与验收。** 让 claim 返回绑定父目录和文件名的对象，后续写入与条件清理复用它，并在发现配置路径变更时显式失败。用两个实例在 claim 与发布之间轮换父目录别名，验证不会出现两把有效锁共同管理一个可见凭据文件。

### F09 · medium · Windows Files 可发出接收方无法解码的目录页

**位置与证据（静态，Windows）。** `crates/ma2a-services/src/files/dir_windows.rs:104–108` 接受所有可转成 Unicode 的文件名。`crates/ma2a-services/src/files/ops.rs:55–76` 将它们排入分页结果；`crates/ma2a-core/src/service/files_result.rs:73–81` 编码时只检查条目数。解码在第 109、115 行对条目名和游标都限制为 255 个 UTF-8 字节，常量见 `crates/ma2a-core/src/service/names.rs:10`。

**触发与影响。** 本地导出的目录含有用 100 个基本汉字命名的文件。此文件名为 100 个 UTF-16 码元、300 个 UTF-8 字节，可以存在于普通 NTFS 卷，却超过协议上限。只要该文件出现在结果页，客户端就会拒绝整页，连同页可寻址的文件也列不出来。

**修复与验收。** 构造候选页前过滤协议无法寻址的名字，或明确修改协议并同步所有编码、游标及客户端约束。Windows 集成测试在同一目录混合可寻址与超字节上限的名称，要求所有返回页可被现有客户端解码，合法兄弟项可跨页列全。

### F10 · medium · 上传恢复依据文件名删除对象，限制文档给出的归属保证不成立

**位置与证据（静态）。** `docs/limitations.md:64–71` 承认恢复后会按旧路径删除，却断言同名条目只能是此次上传的副本。`crates/ma2a-services/src/transfer/upload.rs:72–83` 在排他创建成功之前就持久记录路径。`crates/ma2a-runtime/src/store/services.rs:103–109`、`:162–172` 只检查文件名形态并调用 `remove_file`，没有比较创建结果、文件身份或内容。

**触发与影响。** 同一账户在恢复目录时保留了原暂存名，之后将其替换为需保留的文件；或者暂存同名项本来就存在，Runtime 在提交日志之后、取得文件之前中断。下次启动可删除这个从未归属于该次上传的对象。限制 Files 协议保留名影响远端命名能力，不能证明本地恢复后的文件归属。

**修复与验收。** 至少纠正文档，明确恢复前需要隔离同名文件。实现可在持久日志记录可核验的对象身份，对无法证明归属的对象保留并报告。验收应覆盖已登记但未创建、原路径恢复为不同对象、正常清除自身残件三种情况。

### F11 · medium · 并行 E2E 运行即使分开证据目录，也会相互覆盖 JUnit

**位置与证据（静态）。** `scripts/run-e2e.sh:5–6` 允许独立输出目录，却硬编码同一个 JUnit 源。第 104–115 行每轮先删除共享 JUnit，运行 nextest，然后复制到本轮目录；第 118–126 行只核对用例名。`.config/nextest.toml:30–31` 为 ci profile 配置了同一个报告名。

**触发与影响。** 同一工作树的两次运行采用不同 `MA2A_E2E_OUTPUT`，在生成或复制 JUnit 时相遇。其一可能复制失败，也可能将另一轮的 JUnit 与本轮 JSONL、日志配对；两轮用例集合相同，清单比较无法识别错配。还有一个直接后果：显式改变 Cargo target 目录时，脚本仍到默认位置取 JUnit。

**修复与验收。** 给每次运行独立的 nextest 输出位置，并从实际配置读取报告路径；也可在共享源报告的整个生成与复制区间做排他保护。用两个独立输出目录并发运行带可辨标识的测试夹具，检查每组报告都来自自身运行；另验证非默认 target 目录。

### F12 · low · E2E 次数超过 shell 整数范围时，会先清掉上一次证据再失败

**位置与证据（静态及只读实测）。** `scripts/run-e2e.sh:14–16` 检查十进制字面形式；第 64–67 行清理此前证据，第 104、209–212 行才按 shell 整数比较。单独执行生产脚本的参数判断后，用本机 `sh` 比较一个 24 位正整数，参数检查通过，数值比较返回状态 2。没有运行删除或 E2E 阶段。

**触发与影响。** 在复用证据目录时将 `MA2A_E2E_RUNS` 误设为过长正整数。原始格式检查成功，旧报告先被移除，循环条件出错，最后的计数验证失败。此次没有运行任何 E2E，用于诊断的上一轮证据也已失去。

**修复与验收。** 在触碰输出目录前检查实现支持的数值范围，并说明最大值。分别输入最大值、最大值加一、过长数字、零与正常值；无效输入须在清理前退出且保留上一轮证据的摘要。

### F13 · low · SBOM 校验接受同名、不同内容的发行包

**位置与证据（静态）。** `scripts/generate-sbom.sh:19–24`、`:89` 将归档摘要放入 SPDX 的 `documentNamespace`。`scripts/validate-release-metadata.sh:13–36` 检查若干包、许可证与归档 basename，却从不读取归档字节或比较该摘要。`.github/workflows/release.yml:98–113` 在通过此检查后将 SBOM 用于签证。

**触发与影响。** 两次构建产物名称相同，流水线或本地发布验证误配了另一构建的 SPDX 文档。只要抽查的包仍相同，验证器就会给出通过，后续签证可以将错误的依赖清单绑定到当前产物。此次没有发现已发布版本发生错配。

**修复与验收。** 根据实际归档内容计算 SHA-256，核对文档命名空间中的摘要，再执行现有包集合校验。两份同名不同内容的构建分别应与各自 SBOM 校验通过，交换 SBOM 时必须在签证前失败。

### F14 · low · Agent 工作区下载给暂存名追加后缀，部分合法目标名始终失败

**位置与证据（静态）。** `crates/ma2a-runtime/src/agents/tools/files.rs:49–57` 把完整目标名再加上 26 个 ASCII 字节作为暂存组件，第 146–150 行以该名创建文件。`crates/ma2a-runtime/src/agents/tools/workspace.rs:60–82` 接受普通相对目标。`crates/ma2a-runtime/src/agents/tools/files.rs:94–101` 的 Transfer 下载及 `crates/ma2a-runtime/src/agents/tools/agent.rs:175–183` 的制品下载都经过这里。

**触发与影响。** 在单组件上限 255 字节的文件系统中，230 至 255 字节的普通目标名本身合法，辅助名却至少需要 256 字节。已被远端接受的下载在创建本地暂存文件时固定失败，缩短内容或重试均无效。

**修复与验收。** 使用长度固定的随机暂存名，排他创建成功后再管理其清理。分别以 229、230、255 字节的目标运行两个工具，确认产物可按原名保存，且遇到已存在的暂存名时保留原文件。

### F15 · low · A2A 启动通知写失败会遗留本次凭据文件

**位置与证据（静态）。** `crates/ma2a-app/src/a2a.rs:73–76` 先发布令牌，再用 `?` 将就绪通知写入标准输出。第 98 行的条件清理只在运行服务的分支结束后执行。

**触发与影响。** 启动器在读取就绪通知前关闭 stdout 管道。通知写入返回 BrokenPipe，监听器和锁随函数退出而释放，本次写出的凭据文件仍留在预期的发现位置。它对应的适配器已经退出，可使仅按文件存在判断就绪的调用方误判。此项的影响是启动失败后的状态不一致。

**修复与验收。** 从写入成功起就让 guard 负责条件清理，只在内容仍属于本次实例时删除。用返回 BrokenPipe 的通知输出测试初始化，断言启动返回错误、凭据不残留、之后的正常实例可以重新取得锁。

## 第七轮 F01–F22 对照

“关闭”针对上轮的具体触发条件；同一模块发现的其他条件另引本轮编号。Windows 的关闭判断为源码判断。

| 上轮 | 本轮判定 | 依据与边界 |
| --- | --- | --- |
| F01 离场重放 | 关闭；有相邻容量问题 | `a6a8e75` 在迟到离场答复前把 ID 绑定到实际移除代。owner 移除、回复、重新入会、重试的定向用例通过。旧 schema 已答复的边界见 `docs/protocol/enrollment-v1.md`。新绑定的数量见本轮 F07。 |
| F02 同 peer 分片错配 | 关闭 | `539e9de`、`786d99c`。`crates/ma2a-runtime/src/control_sync/exchange.rs:129–141` 在应用前比对本次请求的 Space 集合、数量；拒绝响应使对应请求失败。交换其他分片答复的测试通过。 |
| F03 push 进展描述 | 文档关闭 | `c850cd7`。`docs/limitations.md:55–61` 已写明 push 可能重发同一完整前缀，落后方靠基于游标的 pull 继续。 |
| F04 下载校验与发布 | Unix 原条件关闭；Windows 仍有缺口 | `228d026`、`8ac73cd`。Unix 持有目录并核对父目录和 partial 身份，目录轮换测试通过。Windows 见本轮 F02、F03；另一路 Agent CLI 见 F05。 |
| F05 递归删除下载进度 | 关闭 | `08b2392`。Unix 与 Windows 逐层跳过下载保留目录，继续处理兄弟项并报告非空。Linux 递归删除用例通过。 |
| F06 旧 mixed-case 辅助名 | 文档关闭 | `228d026`。`docs/protocol/services-v1.md:321–328` 说明旧锁、按小写哈希的旧进度、重新下载与人工清理；代码用精确目标名字节生成新 partial 名。 |
| F07 锁名与卷的大小写规则 | 原条件关闭 | `228d026`、`1e8005a`。两类锁均在目标旁的锁目录内使用原 basename，由文件系统判定同名；普通路径的下载与 A2A 用例通过。部署目录别名变化见 F08。 |
| F08 备份对象与 SQLite 打开路径 | 部分关闭 | `d84b540`、`bfe9227` 加上规范路径的祖先权限检查和暂存对象清理保护，固定路径的备份测试通过。检查结果未传给实际打开操作，见 F01。 |
| F09 Windows A2A 创建时的 ACL | 静态关闭 | `1e8005a`、`fcae832`。`crates/ma2a-app/src/a2a/token_file_windows.rs` 在创建随机暂存目录时提供保护性安全描述符，凭据在目录内创建，再发布。此轮未运行原生 ACL 测试。 |
| F10 不同 state 目录各锁同一 token | 固定父目录条件关闭 | `1e8005a` 将锁移到 token 旁，不依赖 state 路径。Linux 锁测试通过。锁与发布间的别名切换见 F08。 |
| F11 删除未认领 A2A 暂存项 | 关闭 | `1e8005a`。暂存目录已存在时尝试下一随机名，只有本次取得的目录进入发布和清理；已有名字保留的测试通过。 |
| F12 Windows Store 所有者检查 | 静态关闭 | `80eccd2`。`crates/ma2a-store/src/permissions.rs:192–208` 检查对象 owner，并经环境传入路径，解决 PowerShell 未取得路径的问题。`6fc0674` 修正原生测试夹具；本轮未运行 Windows PowerShell。 |
| F13 SDK 输入结束后遗留 handler | 对上轮待处理异步任务关闭 | `bb69b69` 将 handler 纳入连接持有的 JoinSet，EOF 主动 abort，写侧失败随集合 drop 取消。已有 EOF、输出失败、末次 ACK 共三项测试通过。取消保证按 Tokio 任务可让出执行权的语义理解。 |
| F14 Process 重启行为说明 | 文档关闭 | `c850cd7`。`docs/limitations.md:72–79` 说明孤立 Process 可跨 daemon 重启存活，并给出了列举、检查、终止流程。 |
| F15 上传清扫归属 | 部分关闭 | `c850cd7` 已说明旧路径可能移除恢复条目，但继续断言该对象必属原上传，和创建、日志、清扫顺序不符。见 F10。 |
| F16 Store 可产生超大 exposure 列表 | 文档关闭 | `c850cd7`。`docs/limitations.md:80–86` 已列出直接写 Store 的途径，知道暴露名时可以逐项 unexpose。分页仍属明确的协议限制。 |
| F17 SBOM 纳入过程宏 | 关闭 | `f252a5a`。实际 Cargo metadata 配合生产 jq 选择器在两个发行目标均保留 `serde` 并剔除 `serde_derive`。与归档内容的绑定见 F13。 |
| F18 E2E 接受全零次数 | 原条件关闭 | `cf2e287` 在清理前拒绝首位为零的输入，涵盖 `0`、`00`。过大的非零值见 F12。 |
| F19 E2E 末尾斜杠 | 关闭 | `cf2e287`。`scripts/run-e2e.sh:17–20` 在复用目录枚举前去掉末尾分隔符，同时保留根目录的语义。 |
| F20 预建目录的归属 | 部分关闭 | `228d026` 保留了不含标记的预建空辅助目录，定向测试通过。有同名标记或已有锁子目录时仍会删除操作者的对象，见 F06。 |
| F21 一次 Store 错误跳过之后的撤权 | 原条件关闭；有相邻时序问题 | `06f5c15` 让 Unavailable 保持未决，下一 tick 即使 revision 相同也重试；相应测试通过。已读授权尚未绑定根时见 F04。 |
| F22 Exec 输出读取及 wait 错误被吞 | 关闭 | `4f16f8f`。`crates/ma2a-services/src/exec/run.rs:45–57` 区分读取错误与 EOF，等待失败映射为 Unknown。两项定向测试通过。 |

## 覆盖与待确认

共确认 **15 项**：critical 0、high 5、medium 6、low 4。F01–F05 涉及错误发布、内容完整性或撤权后仍可访问旧目录；其中 F02、F03 的 Windows 行为仅经源码核实。统计不含已修复的上轮问题。

| 范围 | 深度 | 实际核查边界 |
| --- | --- | --- |
| ma2a-core | 抽查 | SpaceChain 历史及克隆语义、控制响应的 Space 集合、Files 目录项及游标的协议编码与解码。 |
| ma2a-store | 改动深审，其余抽查 | 离场回执事务、备份创建和发布、Unix 权限与 Windows ACL、上传暂存日志。没有逐版本演练数据库迁移或断电恢复。 |
| ma2a-runtime | 改动深审，其余抽查 | 离场答复、同步调度与结果投影、长操作在登记及绑定后的持续鉴权、上传恢复、Agent 工具工作区；抽查 Web 会话仓库复用与 Process 接管边界。 |
| ma2a-services | 下载、Files、Exec 深审，其余抽查 | Unix/Windows 目录锚定、租约与清理、分页、递归删除、Transfer 摘要、Exec 读与 wait 错误；抽查 Shell 与 Proxy 入口和超时。 |
| ma2a-app | 改动及相邻入口深审 | A2A 锁、暂存、凭据就绪与退出，Transfer CLI 与本地、远端 Agent 制品路径。没有遍历全部命令行组合。 |
| ma2a-agent、ma2a-service | 改动深审、Hosted SDK 抽查 | 前者核查 SDK 待处理任务、输入 EOF、输出错误和末次 ACK；后者抽查宿主通知、调用任务及断链时限。 |
| ma2a-net | 抽查 | 控制拨号并发与轮次时限，Windows relay 密钥 ACL 的句柄检查。没有模拟真实 QUIC 中继或网络分区。 |
| web/src | 抽查 | 控制台快照恢复、Space 详情旧响应的世代及链哈希判定、Services 轮询停止后的状态更新。没有运行浏览器测试。 |
| xtask、scripts、CI、docs | 发布与 E2E 证据链深审，其他抽查 | target 矩阵、发布策略、SBOM 生成与验签前校验、E2E 证据复用与 JUnit 归档、协议及限制文档；没有触发 CI、签发或发布制品。 |

**待确认（未计数）。** 上轮保留的“截断 push 同页附带较新 relay 公告”仍缺少能证明生产预算边界和授权切换同时成立的签名页。`crates/ma2a-runtime/src/control_sync/pages.rs` 的选页与 `crates/ma2a-runtime/src/control_sync/pages/staging.rs` 的接收校验值得用同一条真实链对照。现有证据不足以判断能否由正常操作触发同步停滞。

## 实际执行的验证命令与结果

以下均在仓库根目录执行。未构建或启动完整 workspace 测试、E2E 脚本和原生 Windows/macOS 测试。Rust 用例直接调用工作树中已有的 Linux 测试可执行文件，不把缓存二进制的通过视为对本提交的全新构建证明。协议、权限及时序结论另外由 `4af6c24` 源码核对。

**1. 基线、差量、工作树。** 两个引用均为 `4af6c24`，范围内 20 个提交。改动涉及 60 个文件，新增 2591 行、删除 620 行。`git diff --check` 提示第七轮已提交报告的末尾有多余空行。当前工作树和暂存区的已跟踪文件均无差异；历史未跟踪的 `.omo/reviews/` 在审查前已存在。

```sh
git rev-parse --short HEAD
git rev-parse --short origin/master
git rev-list --count 210ac51..4af6c24
git log --oneline 210ac51..4af6c24
git diff --stat 210ac51..4af6c24
git diff --check 210ac51..4af6c24
git diff --quiet
git diff --cached --quiet
git status --short --branch
```

**2. Rust 定向测试。** 依次通过 3、6、6、1、8、3、3、2、8 项，共 40 个不同用例，失败 0。覆盖回执、备份、下载目录与 sidecar、递归删除、控制回复匹配、暂时性授权错误、Agent SDK、Exec 和 A2A。正文所列新时序或新平台验收条件没有计入这 40 项。

```sh
target/debug/deps/departure_receipts-f39c0be4a1100fbe --test-threads=1
target/debug/deps/backup-f075e7ca66fb3636 --test-threads=1
target/debug/deps/download_directory-09ad4dc0882360fc --test-threads=1
target/debug/deps/files_host-240930032b05ace9 a_recursive_removal --test-threads=1
target/debug/deps/download_sidecars-d4498c255f80b4bc --test-threads=1
target/debug/deps/ma2a_runtime-dc29bac738aec82a another_requests_spaces later_tick a_late_departure_answered --test-threads=1
target/debug/deps/sdk_transport-f8df9f81aed26bb5 --test-threads=1
target/debug/deps/ma2a_services-51ad2c21b903b829 a_failed_read a_failed_wait --test-threads=1
target/debug/deps/ma2a-c741e92a56703142 a2a::token_file::tests --test-threads=1
```

**3. SBOM 选择器。** 直接提取生产脚本的 `shipped` 函数，对两个发行目标的离线 Cargo metadata 执行。输出依次为包数、是否含 `serde`、是否含 `serde_derive`。Linux 为 `[309,true,false]`，Windows MSVC 为 `[319,true,false]`。这是对依赖选择的验证，没有生成归档或签证。

```sh
python3 - <<'PY'
import pathlib, subprocess
source = pathlib.Path("scripts/generate-sbom.sh").read_text()
select = "def shipped:" + source.split("  def shipped:", 1)[1].split("  def rust_packages:", 1)[0]
query = "[.] as $cargo | " + select + ' shipped as $s | [.packages[] | select($s[.id]) | .name] | [length, index("serde") != null, index("serde_derive") != null]'
for target in ("x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"):
    raw = subprocess.check_output(["cargo", "metadata", "--offline", "--locked", "--format-version", "1", "--filter-platform", target], stderr=subprocess.PIPE)
    print(target, subprocess.check_output(["jq", "-c", query], input=raw).decode().strip())
PY
```

**4. E2E 参数及目录归一化。** 取生产脚本的相应语句在子 shell 中运行。`0`、`00`、`01` 均返回 2，`3` 返回 0；三种目录拼写均归一为 `evidence`。

```sh
python3 - <<'PY'
from pathlib import Path
from subprocess import run
lines = Path("scripts/run-e2e.sh").read_text().splitlines()
guard = "\n".join(lines[13:16])
trim = "\n".join(lines[17:20])
for value in ("0", "00", "01", "3"):
    checked = run(["sh", "-c", 'runs=$1\n' + guard, "sh", value], capture_output=True)
    print(value, checked.returncode)
for value in ("evidence", "evidence/", "evidence///"):
    normalized = run(["sh", "-c", 'output=$1\n' + trim + '\nprintf "%s" "$output"', "sh", value], capture_output=True, check=True)
    print(value, normalized.stdout.decode())
PY
```

另对 F12 执行仅含前置判断与数字比较的命令。输出含 `guard=0`、`comparison=2`，shell 报整数无效。全程没有为这一检查创建或复用任何证据目录。

```sh
sh -c 'runs=999999999999999999999999; eval "$(sed -n "14,16p" scripts/run-e2e.sh)"; printf "guard=%s\n" "$?"; [ 1 -le "$runs" ]; printf "comparison=%s\n" "$?"'
```
