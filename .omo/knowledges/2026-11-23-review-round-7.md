# MA2A 第七轮代码审查

基线为 `master`、本地 `origin/master` 指向的 `210ac51`。比较范围为 `1d96672..210ac51` 的 16 个提交。表中的上轮编号指第六轮报告。本轮正文按影响编号，验收条件是给后续修改者的检查项。本次实际执行的命令和结果列在验证小节。

正文共 22 项，其中 critical 0、high 5、medium 11、low 6。优先处理下载及备份“校验的对象与发布的对象可能不同”（F04、F08）、Windows 凭据和状态的权限边界（F09、F12），以及撤权后失败的长连接复验（F21）。

## Space 与控制同步

### F01 · medium · 已经回答的离场请求仍可能移除下一次入会资格

**位置与证据（静态）。** `crates/ma2a-runtime/src/store/membership.rs:47` 优先查精确回执，第 63 行的非成员兼容分支却只读历史并返回，第 77 行仅在新移除时登记请求。`docs/protocol/enrollment-v1.md:27` 承诺旧请求不会移除以后的成员资格。

**触发与影响。** owner 主动撤销成员后，仍持旧视图的成员发出离场请求，owner 返回已有移除链。即使从一开始就用 schema 11，这次成功答复也没有回执。成员通过新邀请加入后，同一请求再次到达，owner 会签发一个新移除代。升级前已响应、升级后才首次重试的请求也有代际歧义。上轮两次正常离场各有回执的路径已修。

**修复与验收。** 在回答兼容请求前，把该请求绑定到历史中实际移除发送者的代，不增加 manifest 代数。对升级前无法重建的请求，明确重放边界或让请求携带入会代际。覆盖“owner 撤销、迟到的离场得到应答、重新入会、原请求重试”，断言最后一次返回原哈希且保留新资格。

### F02 · medium · 控制同步把其他分片的答复计作本分片成功

**位置与证据（静态）。** `crates/ma2a-runtime/src/control_sync/scheduler.rs:115` 派发后仅返回 peer 和响应。`crates/ma2a-runtime/src/control_sync/exchange.rs:123` 和 `crates/ma2a-runtime/src/control_sync/pages.rs:196` 仅检查页属于双方共享 Space。响应端则按本次游标生成页，连无变化的 Space 也有一张带 Space ID 的空页（`crates/ma2a-runtime/src/control_sync/exchange.rs:51`、`crates/ma2a-runtime/src/control_sync/pages/pull.rs:14`）。

**触发与影响。** 同一 peer 跨多个分片时，若对端误把第一片的合法答复也当作第二片的答复，客户端两次应用同一组页，第二片未覆盖的 Space 仍落后。两次传输都成功时，调度器第 75 行仍将该 peer 列为已同步，指定 peer 的等待者也会得到成功。签名和共享成员鉴权仍会执行。

**修复与验收。** 让每次拨号结果携带请求中的 Space ID 集合，核对返回页与游标的对应关系后再记完成。用两个合法分片交叉返回响应，断言不能记为该 peer 已同步，再用合法的逐 Space 空页验证无变更轮正常成功。

### F03 · low · 限制文档高估了单向 push 的进展

**位置与证据（静态）。** `docs/limitations.md:55` 的标题称每轮追上一页，正文称从对端缺少的最早 manifest 开始。`crates/ma2a-runtime/src/control_sync/pages.rs:56` 从本机历史的首个 manifest 枚举，只有反向 pull 的 `crates/ma2a-runtime/src/control_sync/pages/pull.rs:25` 从游标处跳过已知历史。

**触发与影响。** 对端已经持有完整首个预算页，后面仍有缺口、预算不变时，即使首次手动 push 也可净推进零代。操作者可能把连续成功轮次误当成追赶进度。接受旧 F06 推迟协议变更的决定。

**修复与验收。** 标题和正文注明主动 push 重发固定前缀，落后方有可用地址时可用 pull 从已知代继续。文档示例应能解释“已持整页前缀，再连续发起两轮而代数不动”。

## 下载、Files 与备份
### F04 · high · 下载校验和发布可落在两个不同的文件上

**位置与证据（静态，Unix）。** `crates/ma2a-services/src/client/resume.rs:124` 向打开的文件写入并计算哈希，`crates/ma2a-app/src/commands/remote/transfer.rs:49` 随后调用 `finish`。`crates/ma2a-services/src/client/resume.rs:156` 先关闭该句柄，再按保存的完整路径 rename 或 hard link。`crates/ma2a-services/src/files/ops.rs:125` 允许改名一个普通父目录，命名保护只检查操作显式给出的路径组件。

**触发与影响。** 发布前目标的父目录被轮换，接替它的目录恰有同目标名的另一份续传状态，两份状态因目标 basename 相同而使用相同的辅助名。此时校验针对旧目录的文件，发布却能成功交付新目录中的另一份内容，连随后的清理也作用于新目录。只有父目录改名且原路径未重建时，通常表现为失败。直接禁止访问辅助目录已挡住旧 F12 的直接访问场景。

**修复与验收。** 下载期间持有目标父目录、辅助目录及文件的稳定引用，发布和清理都据此执行；若目标路径不再表示预期目录，应保留续传数据并返回错误。用本地目录轮换夹具在验证后改变父目录，检查成功产物始终与已校验的摘要一致，另一下载的文件仍完整。

### F05 · medium · 递归删除普通父目录会清掉受保护的下载进度

**位置与证据（静态）。** `crates/ma2a-services/src/files/ops.rs:143` 检查用户给出的顶层路径；Unix 的 `crates/ma2a-services/src/files/dir_unix.rs:123` 遍历后代时只跳过特殊目录项和非 UTF-8 名，Windows 的 `crates/ma2a-services/src/files/dir_windows.rs:142` 直接调用整树删除。

**触发与影响。** 在可写 Files 根的子目录中有活动或暂停的下载，对这个普通子目录作递归清理。保留名检查未作用到遍历产生的后代，暂停下载的私有 partial、sidecar 和锁文件可以随树被移除，后续失去续传进度。活动下载的结果取决于平台上的打开句柄；Unix 可直接丢失已打开文件的目录项。

**修复与验收。** 让递归遍历遇到保留目录时拒绝跨入并保留它；按已有 partial-result 约定报告此前已发生的删除。对含有暂停下载的目录执行受控清理，断言两端平台均能按原 offset 继续，普通目录的递归删除仍然成功。

### F06 · medium · 更新后大写目标的旧续传文件失去归属

**位置与证据（版本差异，静态）。** `1d96672` 的 `crates/ma2a-services/src/client/resume/place.rs:29` 对文件名先转 ASCII 小写再计算 partial 的散列。当前 `crates/ma2a-services/src/client/resume/place.rs:30` 散列原始名字，`crates/ma2a-services/src/client/resume.rs:52` 只查新路径；旧的 `.ma2a-part.lock` 也改成了 `.ma2a-lock`。

**触发与影响。** 在旧版中断向 `Report` 的大文件下载，升级后对相同来源和相同目标使用 `--resume`。旧进度找不到，下载从零开始。即便成功，清理的也是新路径，已有内容继续占用磁盘。若两个版本同时运行，锁名不一致也使它们不能共同履行单写者约定。

**修复与验收。** 对旧格式提供明确处置，能够验证目的地和来源时迁移，无法区分大小写归属时向操作者说明并提供安全清理途径。用上一个基线创建混合大小写目标的非零进度，再升级，验证明确迁移或告知旧进度，检查未触及另一大小写目标。混合版本若不支持，应写明需要停止旧客户端。

### F07 · medium · 下载与 A2A 的锁键不总能表示同一个文件系统名称

**位置与证据（静态，依赖卷的名字语义）。** `crates/ma2a-services/src/client/resume/place.rs:39` 只折叠 ASCII。`crates/ma2a-app/src/a2a.rs:90` 规范化父目录，最终文件名保留原拼写，仅 Windows 分支执行 ASCII 折叠。两处都将拼写散列成不同的锁文件。

**触发与影响。** 在不区分大小写的 macOS 卷上，A2A 对仅 ASCII 大小写不同的同一文件就可以给出两个锁。若卷还把一对非 ASCII 大小写或规范等价名字解析到同一个目录项，下载的锁也会分裂。并发操作可同时取得各自的锁，出现成品或凭据被另一写者替换。Linux 的 ASCII 用例验证了相反方向的要求，即不同目标保持各自进度。

**修复与验收。** 让锁名在目标卷上按与目标 basename 相同的名字语义竞争，例如在隔离的锁目录中保留目标名本身，并考虑组件长度；或用稳定目录身份串行化认领。在 CI 先实际探测两个名字是否指向同一文件，再对同一目录项的两个拼写断言只能持有一个 lease；区分大小写卷继续检查进度互不覆盖。

### F08 · high · 备份发布核对的句柄没有绑定到 SQLite 实际写入的对象

**位置与证据（静态；现有发布防替换用例另有实测）。** `crates/ma2a-store/src/backup.rs:30` 保存最初创建的句柄，`crates/ma2a-store/src/backup.rs:40` 却按路径重新建立 SQLite 连接，完整性检查针对该连接。第 106、117 行只比较链接时的名字和最初句柄。第 143 行在检测到替换后仍按名字删暂存文件；第 175–178 行的 Unix 用例明确期望该替代物也被清掉。

**触发与影响。** 在允许目录项变动的备份父目录，若 SQLite 打开时解析到的暂存对象与最初创建的不同，而发布时名字又对应最初对象，两个 inode 检查都能通过。备份可能报告成功而交付空文件，完整性检查实际针对另一个数据库。即使替换持续到发布并引发失败，清理仍可能删除不是这次创建的对象。父目录由备份进程独占时，正常备份和竞争发布的现有测试均通过。

**修复与验收。** 固定暂存目录的身份，让 SQLite 打开、验证、发布和清理都针对同一对象；平台难以提供这种绑定时，把可信父目录作为经过验证的前置条件。用注入文件操作的测试令 SQLite 连接与原句柄指向不同对象，应失败且不删替代物。另将成功的备份重新打开，核对实际业务行和 revision。

## A2A、Windows 权限与 Agent

### F09 · high · Windows 自定义凭据文件先继承权限，再收紧 DACL

**位置与证据（静态，未运行 Windows）。** `crates/ma2a-app/src/a2a.rs:45` 用普通 `OpenOptions` 排他创建暂存文件，第 54 行才按路径调用 `protect_private_file`，第 57 行开始写入。`crates/ma2a-store/src/permissions.rs:77`、第 145 行表明收紧权限需在创建后另起 Windows PowerShell。对比 `docs/protocol/a2a-adapter.md:18` 的共享父目录承诺。

**触发与影响。** 自定义父目录的继承规则允许另一个账户读取新文件。旧 F17 的“最终凭据始终继承宽权限”已修，但在保护生效以前已经授予的读句柄并不会因 DACL 更新而失效，可继续读取随后发布的凭据。默认受保护 state 目录不具备这个前提。

**修复与验收。** 创建对象时直接指定受保护的安全描述符，或在一开始就具备可靠隔离的同卷目录内创建再发布。原生 Windows 验收应在创建时检查安全边界，覆盖共享父目录；仅检查最终文件的 ACL 无法证明该条件。

### F10 · medium · 不同 state 目录对同一 A2A 凭据各自加锁

**位置与证据（静态）。** `crates/ma2a-app/src/a2a.rs:84` 给每个 state 目录建立自己的锁目录。第 96–111 行虽对同一凭据路径算出同一散列，锁文件的父目录仍不同。`docs/protocol/a2a-adapter.md:19` 对同一凭据文件承诺互斥。

**触发与影响。** 同一账户运行两个不同 state 目录的 Runtime，两个 adapter 使用不同监听端口和同一个自定义凭据路径。两个 claim 都能成功，后启动者替换凭据，先启动者的客户读取新内容后鉴权失败。现有单 state 重入和路径别名测试已通过。

**修复与验收。** 让同账户内指向同一凭据位置的认领落到同一受保护锁域，或将可选凭据限定在各自互不重叠的目录中。使用两个独立 state 目录认领同一普通文件，第二次必须在写入前失败，首个 adapter 的凭据需保持可用。

### F11 · medium · 写入 A2A 凭据会先删除未经认领的同名暂存文件

**位置与证据（静态）。** `crates/ma2a-app/src/a2a.rs:44` 替换目标扩展名得到含 PID 的暂存名，第 52 行在排他创建之前无条件 `remove_file`。`claim` 仅保护目标凭据的锁，不证明旁边这个名字属于本次运行。

**触发与影响。** 自定义凭据所在目录本来就有同名普通文件，或 PID 重用后该名已被其他工作使用。启动时会删除原文件；即便后面的权限设置或发布失败也无法恢复其原内容。

**修复与验收。** 选择新随机名并用 `create_new` 认领，碰撞则换名；失败时仅清理本次创建的对象。用可注入名字源使首次候选与既有文件相同，检查凭据成功发布或明确失败，两种结果均须保留原文件。

### F12 · high · Windows Store 把其他账户拥有的对象当作私有状态

**位置与证据（静态，未运行 Windows）。** `crates/ma2a-store/src/permissions.rs:136` 进入 ACL 校验，第 160 行检查 DACL、规则 SID 和权限位，没有检查安全描述符的 owner。相同仓库中，私有 relay 密钥在 `crates/ma2a-net/src/relay_tls_windows.rs:82` 和第 99 行另验 owner SID。Store 用该判定接受 key 文件（`crates/ma2a-store/src/key_store.rs:225`）。

**触发与影响。** 用恢复、复制或预先配置的目录作为现有 state，某个受保护对象由其他普通账户拥有，但两条显式 Allow 分别授予运行账户和 SYSTEM。当前判定会通过。对象 owner 仍能调整 DACL，因而“读取受保护身份材料只限本账户和 SYSTEM”的边界不能由这两条 ACE 单独保证。此项与旧 F14 的权限位遗漏不同。

**修复与验收。** 通过同一对象的安全描述符验证 owner，并明确是否接纳 SYSTEM 所有的对象。原生测试用同一 DACL 搭配受信与不受信 owner，验证后者在加载状态前失败，保留半权限、重复 SID、缺 SYSTEM 的反例。

### F13 · medium · 嵌入式 Agent SDK 返回后，仍有任务等待已经关闭的传输

**位置与证据（静态；既有传输测试已执行）。** `crates/ma2a-agent/src/sdk.rs:177` 将 inbox、取消通知及工具响应通道存入共享表，第 192 行独立 spawn handler 而未保留 JoinHandle。输入结束时第 139 行仅丢弃服务循环自己的 Arc；`crates/ma2a-agent/src/sdk/task.rs:99` 和第 163 行可一直等取消或工具答复。

**触发与影响。** 长期运行的宿主使用公开的 `serve_on`，当一个 handler 正在等待取消或工具答复时输入关闭。任务自己持有 Arc，表中的发送端继续存在，`serve_on` 可以已返回而旧任务仍在运行或等待。宿主如再次连接，会积累无法由原连接回收的工作。独立 CLI 进程随进程退出清理的情况不受同样影响。

**修复与验收。** 在所有退出路径（含输出写失败）关闭该连接的任务槽并取消、回收 handler，区分工具结果未知与正常结束。用内存双工连接让一个 handler 等待工具结果，输入关闭后同时检查服务函数返回、handler 析构和资源释放。现有 `sdk_transport` 两个测试验证写失败退出与 ACK 排空，未断言活动 handler 的生命周期。

### F14 · low · 限制文档误称停止 Runtime 会结束未绑定的 Agent Process

**位置与证据（静态）。** `docs/limitations.md:66` 将 Runtime 停止写成残留 Process 的终点。`crates/ma2a-runtime/src/agents/instance.rs:125` 耗尽重试后只留下日志，`crates/ma2a-app/src/commands/supervisor.rs:24` 分离 supervisor，`docs/protocol/process-v1.md:388` 也明确普通 daemon shutdown 会保留 Process。

**触发与影响。** 清理失败、Agent 已解除定义且 Process 自身继续运行时，按该说明重启 Runtime，后台 Process 仍可存活并被 Process 子系统重新接管。接受旧 F16 推迟持久化清理的理由；这里需修正的是处置指引。

**修复与验收。** 说明重启不能保证终止，列出用 `ma2a process list` 找到残留 Process 并主动终止的途径。用一个跨 daemon 重启仍在运行的 Process 核对操作说明。

## 恢复、发布与 E2E 证据

### F15 · medium · 上传恢复说明漏掉了旧路径被复用时的误删

**位置与证据（静态）。** `docs/limitations.md:60` 将父目录改名的后果描述为只占磁盘。`crates/ma2a-services/src/transfer/upload.rs:73` 记录路径，`crates/ma2a-runtime/src/store/services.rs:103` 按文件名形状筛选，第 166 行按记录路径直接删除，第 172 行忘掉记录。

**触发与影响。** 上传中断后其父目录被轮换，旧位置恢复了一个也含同名条目的目录（例如从目录副本恢复）。启动清扫可删除接替目录中的文件，而在改名后的真实暂存文件上没有执行清扫。这里需要同名条目；只改名且旧位置为空时，文档描述的泄漏成立。

**修复与验收。** 可继续推迟同卷私有 staging 的重构；先补全恢复说明，无法证明旧路径上的文件属于该记录时跳过删除。验收用本地恢复夹具分别覆盖旧路径缺失、旧路径仍是原文件、旧路径已被另一对象占用，第三种需保存现有内容并留下可诊断记录。

### F16 · low · 超限 exposure 数据的来源并不限于历史 CLI

**位置与证据（静态）。** `docs/limitations.md:74` 将超限状态限定为未发布的旧构建。当前的 `crates/ma2a-runtime/src/ipc/server/execute/services.rs:71` 在 IPC 命令层检查条数和编码长度；公开的 `crates/ma2a-store/src/service_exposures.rs:115` 可对现有 Space 直接插入不同 exposure，第 167 行提交前没有对应集合检查。

**触发与影响。** 本地工具通过当前 Store 接口维护数据，再用 Runtime 加载该库或其备份，也能得到 CLI 无法列举、必须预先知道名称才能删除的状态。普通 CLI 新增 exposure 的限额成立，旧 F19 推迟分页的协议理由成立。

**修复与验收。** 若可列举是全库不变量，在 Store 写入事务中也应实施预算；若该接口有意允许维护操作，收窄文档承诺并给出按名称恢复的本地入口。以当前 Store API 连续写入不同条目，验收最后写入被拒，或运维文档所述入口确能枚举并缩减集合。

### F17 · low · 归档 SBOM 仍将仅构建时使用的过程宏列为随包分发的 Rust 组件

**位置与证据（实际探针与静态）。** `scripts/generate-sbom.sh:28` 声明要生成归档实际分发的依赖，第 40 行只判断 Cargo 边是否为 normal，没有检查包的 target 类型。对两个目标的锁定 Cargo metadata 实际运行生成脚本的 `shipped` 选择器，Linux 的 349 个包中有 30 个包含 proc-macro target，Windows 的 360 个中有 31 个；两者都将纯过程宏 `serde_derive` 列为分发组件。探针只输出计数，不生成或签发归档。

**触发与影响。** 每次制作这两个目标的二进制 SBOM 都会发生。宏展开器在构建主机编译期运行，普通依赖边并不能证明它作为运行组件被打进归档。清单消费者会把仅影响构建链的组件也当作随二进制分发；上轮跨平台包混列问题确有改进。

**修复与验收。** 先明确 SBOM 的口径。若列运行组件，根据实际目标构建单元区分 host 过程宏和目标制品，同时保留经独立运行路径到达的普通库；若覆盖构建来源，则明确身份与依赖关系。用纯过程宏、普通运行库和单平台依赖各设一个验收断言，不以排除 `xtask` 作为充分条件。

### F18 · medium · E2E 次数写成 `00` 时可以零次运行通过

**位置与证据（实际局部探针与静态）。** `scripts/run-e2e.sh:12` 只把精确字符串 `0` 拒绝，第 98 行按整数判循环，第 203–206 行将完成记录、产物数与整数次数比较。抽取实际 guard 和循环条件在 `sh` 下执行，`00` 被接受且循环零次；`0` 返回 2，`1` 和 `3` 分别循环一次和三次。

**触发与影响。** 自动化带前导零传入零次数，inventory 成功后不跑任何测试、不生成单轮证据，末尾四个零计数仍可全部相等并成功返回。CI 当前固定传 3，受影响的是复用同一脚本的配置入口。

**修复与验收。** 在接触证据目录前验证可表示的正整数，明确前导零规则。对 `0`、`00`、空的显式值（按约定处理）、`1` 和 `3` 检查状态；通过的运行次数必须至少产生一组 JUnit 和证据。

### F19 · low · E2E 输出目录带尾随斜杠时，第二次运行拒绝复用

**位置与证据（实际局部探针与静态）。** `scripts/run-e2e.sh:41` 从 `find` 取路径，第 52–53 行用原始 `output` 再拼斜杠作前缀。本机用同一真实目录的两种拼写运行生产前缀判断，不带尾斜杠时通过，带尾斜杠时被拒。

**触发与影响。** `MA2A_E2E_OUTPUT=target/e2e-evidence/` 首次可正常写入，第二次目录含标记及证据文件时，GNU find 返回的条目路径只有一个分隔符，脚本期望两个。合法目录被判为不属于脚本，测试无法启动。

**修复与验收。** 在进行一致的路径操作前规范化尾随分隔符，或用目录句柄及相对 basename 比较，保留拒绝符号链接和未知文件的规则。对两种拼写连续复用标记目录，验收结果应相同；任何拼写下的未知文件都应保留。

### F20 · low · 同用户预建的空下载目录仍会在结束时消失

**位置与证据（静态）。** `crates/ma2a-services/src/client/resume/place.rs:83` 接纳已经存在且权限合适的目录，却没有记录创建归属。第 119 行在清理时总是尝试删除空目录。`crates/ma2a-services/src/client/resume.rs:172` 和第 186 行分别在发布和丢弃后调用它。

**触发与影响。** 操作者事先创建了一个自己的空 `0700` 目录，例如为下载预置特定属组或 ACL。成功完成最后一个下载后，预设目录消失，下次重新创建也可能丢失定制属性。旧 F09 中预建 `0755` 目录导致的信息暴露已经关闭，遗留的是空目录的所有权。

**修复与验收。** 只删除能证明由下载系统创建的目录；无法区分时保留空目录。用预建私有目录完成及放弃下载各一次，断言目录身份和属性保留；仍对非私有目录返回错误。

## 独立复核中的长连接与 Exec

### F21 · high · 撤权后的长连接复验遇到一次 Store 读取失败，就可能跳过后续检查

**位置与证据（静态）。** `crates/ma2a-runtime/src/stream_host/reauthorize.rs:149` 在逐项鉴权之前就把 revision 写入 `seen`；第 163 行遇到 `Unavailable` 时既不取消，也不保留重试标记。`crates/ma2a-runtime/src/store/services.rs:73`、第 80 行把读取成员、exposure 的失败也映射为 `Unavailable`，该错误并不限于 daemon 正在退出。

**触发与影响。** 一个已运行的 Shell 或 Proxy 对应的授权被撤回。下次轮询读取新 revision 成功，但读取鉴权数据临时失败。即便 Store 马上恢复，只要没有下一次持久变更，第 152 行仍每次跳过复验，原流可以继续运行到自身结束。新开的流仍按当前授权检查。

**修复与验收。** 只有所有需复验的操作已得到确定决定才能标记该 revision 完成，或逐操作记下未完成项并在下一个 tick 重试。用本地 Store 夹具令撤权后的第一次权限查询不可用、第二次恢复，保持 revision 恒定，断言原流在下一轮复验被取消。

### F22 · medium · Exec 把进程等待或输出读取失败当作正常退出或 EOF

**位置与证据（静态）。** `crates/ma2a-services/src/exec/run.rs:40` 将 stdout/stderr 读取错误变为零字节，第 105 行按 EOF 移除该管道。第 97 行在子进程 `wait` 出错时填入 `ExitStatus::default()`，第 144 行再按普通状态码解释它。初始结束原因为 `Completed`（第 77 行）。

**触发与影响。** 在子进程输出管道发生读取错误、或宿主无法取得子进程退出状态时，远端可收到被截断的输出却没有流错误，或者收到合成的成功状态。外部调用者可能依此继续一项实际结果未知的操作。

**修复与验收。** 区分零字节和读取错误，保留 `wait` 的错误并使用现有 `Termination::Unknown` 或明确失败结果。用返回短输出后报错的 reader、返回错误的等待接口验证两条路径，均不可给出“完整输出且成功”；正常零退出和正常 EOF 继续通过。

## 实际验证与覆盖

以下命令在仓库根目录执行。Rust 共 24 个不同用例通过，重复执行的用例只计一次。Web 为 8 个用例通过。仅对测试实际覆盖的条件作动态结论。

```sh
cargo test --locked --offline -p ma2a-store --test departure_receipts --test space_members_full --test space_history_limit --test backup --color never -- --test-threads=1
cargo test --locked --offline -p ma2a-services --test download_directory --color never -- --test-threads=1
python3 .omo/reviews/2026-11-23/focused.py
python3 .omo/reviews/2026-11-23/check.py
(cd web && bun run test src/api/coordinator.test.ts src/api/snapshot-stream.test.ts)
```

- Store 四个集成目标共 12 项通过，涉及备份重开与并发、回执及撤销事务、容量预留、满员时邀请仍未消费。下载目录 4 项通过，包含现有权限过宽目录、保留目标名和 ASCII 大小写隔离。
- `focused.py` 另外执行 8 项，通过数分别为 runtime 3、app 2、store 1、agent 2。包括上轮离场重放、失败与空控制轮、所有业务名额占满时的生命周期调用、正常 A2A 锁冲突和替代暂存对象的拒绝。结果在 `.omo/reviews/2026-11-23/focused.txt`。
- `check.py` 在内存中用两个 target 的实际 Cargo metadata 运行生产 SBOM 选择器，并单独执行 E2E 的次数和路径判断。结果在 `.omo/reviews/2026-11-23/check.txt`。这些检查没有启动 E2E 脚本。Web 用例覆盖恢复流程与快照碎片解码。
- 本轮环境为 Linux。Windows ACL、大小写折叠卷、数据库读取瞬断、备份路径轮换和后台脚本的整轮结果均未在本次审查中实测。未运行全 workspace 测试、完整 E2E、归档签发或远端 CI。

| 范围 | 深度 | 核查内容与边界 |
| --- | --- | --- |
| ma2a-core | 成员与控制帧深审，其余抽查 | genesis owner、manifest 撤销历史与成员集合、成员策略上界、游标与响应页的编码校验、入会分页。 |
| ma2a-store | 改动深审 | 容量、入会、回执事务、SQLite 备份的对象身份与发布、Unix 权限和 Windows 检查、exposure 写入。历史迁移没有逐版本执行。 |
| ma2a-net | 抽查 | 控制流的收发、重试、选 peer、回包生命周期和 relay 密钥的原生 ACL 验证。未模拟 QUIC 网络背压。 |
| ma2a-runtime | 涉及路径深审，其余抽查 | 离场 actor 到 Store、控制调度和投影、入站服务持续鉴权、上传清扫、Agent 启动与 Process 关联、IPC 生命周期。Web 鉴权与托管协议入口抽查。 |
| ma2a-services | 下载与 Files 深审，其余抽查 | helper、lease、目录遍历及递归、发布、Transfer journal；另查 Exec 结束状态、Shell 回收、Proxy 连接限时和 supervisor 生命周期。 |
| ma2a-app | A2A 凭据深审，其余抽查 | serve、claim、凭据写入与删除，下载 CLI、supervisor 和两处 Agent 测试改动。 |
| ma2a-agent、ma2a-service | SDK 生命周期抽查 | Agent 通道、工具结果及关闭；对照 Hosted SDK 调用帧和服务断开时的任务回收。未逐个运行第三方示例。 |
| web/src | 抽查 | 快照解码、恢复协调器、页面轮询及停止后的结果处理；两个测试文件已执行。未运行浏览器交互或逐页视觉审查。 |
| xtask、scripts、CI | SBOM 选择与 E2E 证据脚本深审，其他抽查 | 归档构建、清单生成、校验到证明的工作流，E2E 名称识别、清理与完整性计数；另查 xtask 发布策略、版本固定检查及 native 测试接线。 |
| docs | 对照改动深审 | 结合架构、安全、存储、协议、Known Trade-offs 和前序报告核对正文。未逐行审阅全部文档。 |

## 第六轮 F01–F24 对照

“原场景关闭”指第六轮列举的触发条件；相邻条件有独立编号时在同一行列出。

| 第六轮 | 结论 | 当前证据或剩余边界 |
| --- | --- | --- |
| F01 | 部分关闭 | `6f6d442`。两轮正常离场各取原请求回执的测试通过。无回执的成功应答与升级边界见本轮 F01。 |
| F02 | 关闭 | `47cdcc1`。`space_capacity.rs` 用非 owner ID 真子集判定收缩；Core 再要求每个移除都有撤销历史。等人数换人和真正批量移除已在容量测试中区分。 |
| F03 | 关闭 | `0014c4b`。`crates/ma2a-store/src/enrollment.rs:178` 先处理已消费的同请求重试，第 224 行检查策略容量，第 260 行才消费邀请；`crates/ma2a-runtime/src/error.rs:93` 将满员分类为请求拒绝。满员用例通过。 |
| F04 | 原场景关闭 | `6f6d442`。`crates/ma2a-runtime/src/departure_actor.rs:182` 在调用 Store 之前确认只有 genesis owner 对非自己的请求作答。静态复核。 |
| F05 | 原场景关闭，有相邻缺陷 | `ddd4756`。全体 peer 均有传输失败分片时保留已提交进展，失败轮与真正空轮的 actor 测试通过。收到错误分片的合法答复见本轮 F02。 |
| F06 | 接受推迟，文档需修正 | 反向拉取的协议理由成立；当前主动端没有按接收进度续推的依据，见本轮 F03。 |
| F07 | 关闭 | `c73cde7`。`crates/ma2a-store/src/backup.rs:109` 对 hard-link 错误直接失败，无覆盖式 rename 回退。通常文件系统上的并发测试通过；不支持硬链接的路径仅静态核查。 |
| F08 | 关闭 | `c73cde7`。暂存目录为与目标文件名无关的固定长度随机名。接近单组件长度上限的目标备份实测通过。 |
| F09 | 权限泄露原场景关闭，目录归属部分关闭 | `b1c8d13`。Linux 的预建宽权限目录被拒。预建私有空目录的生命周期见本轮 F20。Windows 的 helper 代码明确使用父目录继承策略。 |
| F10 | 原来的进度碰撞关闭 | `b1c8d13`。区分大小写的目标使用不同 partial，ASCII 拼写共用 lease，均有实测。旧格式升级、其他卷名字语义分别见本轮 F06、F07。 |
| F11 | 关闭 | `b1c8d13`。`crates/ma2a-services/src/client/resume/place.rs:55` 在建目录前拒绝目标 basename 等于保留名，混合大小写用例通过。 |
| F12 | 直接定址关闭，祖先目录操作部分关闭 | `b1c8d13`。显式远端组件检查和直接将私有目录作为 root 的检查都已加入。发布时目录轮换和递归清理见本轮 F04、F05。 |
| F13 | 接受推迟 | `crates/ma2a-runtime/src/control_sync.rs:145` 仅将当前有效的成员地址纳入待拨集合；`docs/limitations.md:48` 对失去新鲜地址后的两端行为描述准确。 |
| F14 | 原权限位问题关闭 | `5bc4fa8`。静态核对两个不同 SID、Allow 和全部 FullControl 位要求；权限位之外的 owner 检查见本轮 F12。 |
| F15 | 接受重构推迟，文档低估恢复影响 | 单纯改名造成泄漏的描述正确，原路径复用见本轮 F15。 |
| F16 | 接受持久化方案推迟，需更正处置说明 | 释放重试有界，保护并发绑定的理由成立。daemon 停止与 Process 终止的区别见本轮 F14。 |
| F17 | 部分关闭 | `243e563`。写凭据前及写失败时均有检查和清理；Windows 文件创建到保护生效之间的权限边界见本轮 F09。 |
| F18 | 同 state、直接锁名碰撞关闭 | `243e563`。隔离锁目录和同 state 独占用例通过。跨 state、卷上的路径别名和暂存文件归属见本轮 F10、F07、F11。 |
| F19 | 接受分页推迟，状态来源需补充 | IPC 路径的尺寸检查及超限时的 conflict 保留，当前 Store 的直接写入见本轮 F16。 |
| F20 | 原来的近似名误删关闭 | `d1da8a5`。固定名、纯数字及固定后缀逐一检查，再遍历已列出的条目。含正常尾随分隔符的目录复用见本轮 F19。名称清单不提供并发写者之间的互斥。 |
| F21 | 目标平台过滤关闭，分发口径部分关闭 | `d6be7a1`。两组目标选择结果已实际核对，host 过程宏仍被归入随包组件，见本轮 F17。 |
| F22 | 关闭 | `d1da8a5`。`scripts/run-e2e.sh:34` 纳入 `.tests.txt.unsorted`，对应第 114 行实际写出的中间文件。静态核查。 |
| F23 | 关闭 | `7a16f03`。提交前核对请求人在旧链有成员资格、新链已移除且有撤销；写链和回执在同一事务。两个 Store 回执用例通过。 |
| F24 | 部分关闭 | `c73cde7`。Unix 下链接前、后核对文件身份，单次替换的拒绝测试通过。未覆盖 SQLite 重新开名这一阶段；非 Unix 的 `holds` 只查文件类型，见本轮 F08。 |

### 额外三个修复主题

| 提交 | 结论 | 验证范围 |
| --- | --- | --- |
| `871b68f` | Windows PowerShell 子进程在启动前移除 `PSModulePath`，处理了从 PowerShell 7 继承不兼容模块搜索路径的原因。 | 静态检查 `crates/ma2a-store/src/permissions.rs:169` 与 `.github/workflows/ci.yml:34`。本轮没有原生 Windows 运行结果。 |
| `2b4f7f0`、`c595a28` | 授权传播用只读 task 查询等待预期的拒绝类别改变；最终断言容许 30 秒任务自然完成，同时保留了外来 cancel 必须返回 `unknown_task` 的独立断言。 | `crates/ma2a-app/tests/agent_e2e/adversarial.rs:35`、第 88、90、123 行。静态检查两项竞态修复；本轮未运行该 E2E。 |
| `87875a6` | 在 30 秒期限内移除已结束的 stalled 调用，补至业务上限，观察实际 permit 耗尽后才发 ping。 | `crates/ma2a-runtime/src/ipc/server_tests/lifecycle.rs:115`。定向用例通过；没有重建原 CI 高负载。 |

## 待确认

- 截断的 push 同页附带本地 relay 公告时，发送端基于最新授权，接收端按实际收到的链前缀校验。`crates/ma2a-runtime/src/control_sync/pages.rs:74`、`crates/ma2a-runtime/src/control_sync/pages/staging.rs:109`。本轮没有形成同时满足签名大小、授权切换和剩余页预算的实际样本，未计数。
- 控制响应在传输持续背压时的入站 permit 回收上界。`crates/ma2a-net/src/control/handler.rs:89`、第 125、147 行。应用层写响应没有单独的全程时限，现有证据不足以确定底层连接在受支持配置下的实际回收时间，未计数。

