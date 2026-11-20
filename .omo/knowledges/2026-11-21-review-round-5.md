# MA2A 第五轮代码审查

基线为 `8ded423`，与本地 `origin/master` 相同。复核范围为 `9914f0e..8ded423` 的 27 个提交、第四轮报告及处理表。F 编号表示当前版本仍需处理的独立问题；旧 Q 编号用于追溯，计数不重复。证据区分源码控制流、实际执行的测试和离线探针。历史提交均使用本轮核实的短 SHA。

## 身份、容量与控制同步

### F01 · medium · 重新入会后，旧离场请求仍能撤销新成员资格（Q01 部分关闭）

**位置与静态证据。** `crates/ma2a-runtime/src/departure_actor.rs:135` 丢弃收到的 RequestId，`:187` 仅在当前不在会时选择历史前缀，当前仍是成员则在 `:193` 发起新撤销。`crates/ma2a-store/src/enrollment.rs:216` 在重新入会时清除旧撤销。当前仍处于离会状态的成员不会再从重试看到撤销后的历史，这部分修复成立。
**触发及影响。** 同一已认证 Endpoint 离场、通过新邀请重新加入，此后到达 authority 的旧离场重试会造成第二次签名移除。传输认证保证只能涉及请求者本身；本机 IPC 的请求缓存不能充当 authority 的去重记录。
**修复与验收。** 按 Space、Endpoint 和 RequestId 持久保存离场结果及撤销代际，与成员变更一起提交。用两个成员资格周期验证重放第一次回执时，第二次资格、链头和 revision 均保持原值。

### F02 · high · 历史容量拒绝沿 enrollment 分支退出整个 Runtime actor（Q03 回归）

**位置与静态证据。** `crates/ma2a-store/src/enrollment.rs:238` 的容量准入可返回 SpaceHistoryFull；`crates/ma2a-runtime/src/enrollment/owner.rs:58` 把 Store 错误交给 absorb_background。`crates/ma2a-store/src/error.rs:73` 的临时故障分类不含此错误，`crates/ma2a-runtime/src/actor.rs:95` 将其向外返回，`crates/ma2a-runtime/src/actor/run.rs:133` 以问号退出 actor 主循环。
**触发及影响。** 拥有有效邀请的正常加入者在普通历史额度已用完时兑换邀请。该事务正确地没有提交，但处理这个请求的 Runtime actor 同时停止，其他 Space 的命令和网络工作也受影响。类似的远端离场容量错误经过 `crates/ma2a-runtime/src/departure_actor.rs:145`。
**修复与验收。** 把容量耗尽作为明确、无副作用的业务拒绝在请求边界处理。针对已达第 239 代的 authority，验证一次正常入会被拒后邀请仍未消费、同一 daemon 的后续健康查询和另一 Space 的请求仍可完成。

### F03 · high · 固定 16 个预留代不足以撤回所有在任成员（Q03 剩余设计缺口）

**位置与静态证据。** 默认策略允许 64 人（`crates/ma2a-core/src/policy.rs:126`）；固定预留 16 代（`crates/ma2a-store/src/space_capacity.rs:15`、`:29`）；CLI 及远端离场所用撤销入口每次移除一人（`crates/ma2a-runtime/src/store/membership.rs:37`）。授权位变更保留原撤销集合（`crates/ma2a-runtime/src/store/grant.rs:95`），也不能用预留代来收窄旧授权。
**触发及影响。** 第 239 代仍有至少 17 位非 owner 成员。16 次合法离场或移除后，第 17 位仍在旧 Space 内，owner 再也无法给该成员签署单独撤销。创建新 Space 并不会撤回其他 Endpoint 依然接受的旧 Space 授权。嵌入式 owner API 支持提前批量撤销（`crates/ma2a-runtime/src/actor/handle.rs:231`）；达到第 255 代后，批量更新也没有余量。另一个更早的边界是 64 条历史撤销：依次加入、移除 64 个不同 Endpoint 后，再接纳一位新成员仍合法，但单独移除它要产生第 65 条撤销，被 `crates/ma2a-core/src/manifest.rs:46` 拒绝。
**修复与验收。** 新签名须满足“剩余代数足以逐一撤销全部非 owner 成员”，同时检查撤销集合自身的容量；为已经突破这个不变量的历史提供可认证的封存或迁移。验收须从多成员近上限状态逐人移除直到只剩 owner，另测升级前已满的状态。

### F04 · medium · 同一 peer 的分片会互相挤占名额，部分失败仍向显式同步报告成功（Q04）

**位置与静态证据。** `crates/ma2a-runtime/src/control_sync.rs:163` 按 64 切片且轮转出队，但 `crates/ma2a-runtime/src/control_sync/scheduler.rs:113` 只限制全局并发 8；入站每个 Endpoint 只允许 2（`crates/ma2a-net/src/control.rs:21`）。失败分片在 scheduler 第 66 行跳过，任一成功分片即在第 81 行登记整个 peer；`crates/ma2a-runtime/src/control_actor.rs:158` 按此集合完成等待者。
**触发及影响。** 129 个共同入选的 Space 可以对同一 peer 同时提交三个请求。容量拒绝引起的传输失败可重试，但次数有限；只要其中一个分片成功、另一个持续失败，指定 peer 的同步命令仍报告成功。即便只有两个分片，部分链路故障也会触发相同误报。未断言每次三分片交换都失败。
**修复与验收。** 每个 peer 限制并发并按分片记录完成情况；只有全部计划分片成功才通知该 peer 的等待者成功。对 65、129 个 Space 分别注入单分片失败及恢复，并检查失败反馈、重试范围和其他 peer 的公平性。


### F05 · medium · 混合长度 manifest 会形成接收端必拒的分页（上轮待确认一，已确认）

**位置。** `crates/ma2a-runtime/src/control_sync/pages/pull.rs:24`、`crates/ma2a-runtime/src/control_sync/pages/builder.rs:50`、`crates/ma2a-runtime/src/control_sync/pages.rs:109`、`crates/ma2a-runtime/src/control_sync/pages/staging.rs:85`。放不下当前代时继续尝试下一代；Q02 新增的提前返回发生在遍历所有 manifest **之后**。push 在 pages.rs 第 57 行也使用相同选择方式。
**触发及探针证据。** 64 个请求游标，目标链的接收游标为 1。测试历史包含入会、反复调整一位成员的 Files 授权，以及一位长标签成员离场。实际每页预算 16,348 字节，连续前缀占 16,043；第 47 代需 357 字节，第 48 代只需 285。选择第 48 代后，整份 64 页响应可以编码、解码，但接收端在前缀上调用原版 SpaceChain::apply 返回 skipped generation。签名、成员变化、链导出和响应编解码均使用当前 ma2a-core；探针按照当前页选择规则重建这一边界，其他页为空的测试夹具，未运行 64 个真实 Space 的网络拓扑。接收端先暂存整批，拒页后没有新游标；同组游标及预算的下一轮会重复拒绝，外部状态变化才可能打破循环。
**修复与验收。** manifest 列表在第一个放不下的代就停止，置 more；push 同样保持连续前缀。用归档中的真实签名长度验证首轮接收到第 46 代、随后两代均可送达，每一轮的游标单调前进。

### F06 · medium · 全部已知地址过期的两端没有重新引导入口（Q05 仅文档）

`crates/ma2a-runtime/src/control_sync.rs:144`、`:155`，`crates/ma2a-runtime/src/control_sync/scope.rs:98`，`crates/ma2a-core/src/address.rs:20`。**静态。** 两端离线超过最长 10 分钟且无第三方提供新地址，重新发布各自的地址并不会让对方进入候选。显式同步仍从空候选集中选择。限制文档已准确描述现象；无需为解决此事信任过期的 relay 权限。较小方案是支持显式导入新鲜、已签名且属于当前 Space 成员的地址，再执行一次现有同步。验收需清除瞬态路由，验证恢复成功及原有签名、期限、资格检查全部生效。

## 持久化、文件与进程生命周期

### F07 · medium · 在线备份仍可能抢占同一最终文件或留下不完整成品（Q06）

`crates/ma2a-store/src/backup.rs:15`、`:20`、`:24`。**静态。** 两个独立 Repository 并发备份到同一尚未存在的路径，存在性检查都可成功，之后都用普通可创建方式打开同一文件。单个调用在创建后失败也留下最终路径，造成普通重试被拒。范围为公开 Store API，本轮没有发现 CLI 自动并发调用。建议在目的目录排他创建私有临时文件，完成 integrity_check 并关闭后，以同卷原子 no-replace 操作发布。验收应验证竞争者只有一个成功，任一阶段发生 I/O 失败后最终名要么不存在，要么始终是第一份完整可恢复的备份。

### F08 · low · Windows 存储权限谓词仍接受不完整的预期 ACL（Q08）

`crates/ma2a-store/src/permissions.rs:134`，`docs/storage.md:15`。**静态。** 两条显式 Allow 且 SID 均在白名单内即可进入掩码检查；各自的权利与 FullControl 有任意交集就通过。当前用户与 SYSTEM 只有部分权限时会误报合规，也未要求每个预期 SID 各出现一次。建议按 SID 分组逐一检查完整必需掩码；用原生 Windows 的精确 ACL、部分授权及缺少其一三种状态验收。本轮未运行原生 Windows。

### F09 · high · staging 名称检查在大小写折叠的文件系统上不能保证上传完整性（Q12、Q13）

`crates/ma2a-services/src/files/resolve.rs:40`、`:51`，`crates/ma2a-services/src/files/dir_windows.rs:146`，`crates/ma2a-services/src/files/commit.rs:58`。**静态，结论限于具有该名称解析语义的卷。** 保护使用区分大小写的 starts_with；底层卷可以将不同大小写的拼写解析为同一目录项。合法写者的并行目录变动仍可能作用于上传暂存对象，发布时按名称找到的对象可能与核对的流哈希不同。精确前缀的 Linux 回归测试覆盖不到此条件。建议把暂存放在远端不可寻址的命名空间；短期应至少令 ASCII 前缀比较与卷语义兼容。验收比较并发目录操作期间成功回执与实际文件的摘要，覆盖大小写不敏感的 NTFS 或 APFS 及普通 Linux 文件系统。

### F10 · medium · 恢复清扫仍只有路径，没有创建凭证（Q13）

`crates/ma2a-services/src/transfer/upload.rs:76`、`:79`，`crates/ma2a-runtime/src/store/services.rs:165`。**静态。** 日志先提交，排他创建在后。两步之间进程中断时，同名本地旧文件会被下次启动当作本次上传文件删除；在上传期间重命名父目录也会使记录路径失去对原目录的指向。限制远端文件名不能建立对象归属。建议将上传对象建立于 Runtime 独占目录，或记录可校验的目录和文件身份，恢复时跳过未能证明归属的对象。验收覆盖日志已写但创建未完成、以及父目录改名后原位置已换为别的目录。

### F11 · high · 下载仅凭标头或空文件推断归属，仍可误删或截断其他文件（Q14）

`crates/ma2a-services/src/client/resume/claim.rs:19`、`:35`、`:55`，`crates/ma2a-services/src/client/resume.rs:66`、`:108`，`crates/ma2a-services/src/client/resume/lease.rs:44`。**静态。** 旧 meta 只要以固定标头开头，旁边任意非空 part 就被视为本轮可以接管。曾经有合法 meta，后来操作者把另一个文件放到 part 位置，普通非续传操作就会截断它。单独的空文件也可被直接接管或作为 lock 删除。两个不同下载目的地若恰好是另一下载的辅助名，各自取得的是不同 lease。Unix 的 inode 检查与四次重试已恢复，可协调同一目的地上的配合写者，无法证明其他辅助文件的归属。建议将状态和租约迁至私有命名空间；遗留格式保守迁移，无法核对时拒绝操作。验收保留无关空文件、带合法旧 meta 的替换文件，以及名称相互覆盖的两个合法下载结果。

### F12 · medium · 下载在写入临时 meta 时中断，会拒绝自己的恢复文件（Q14 回归）

`crates/ma2a-services/src/client/resume.rs:50`、`:108`，`crates/ma2a-services/src/client/resume/claim.rs:17`、`:40`。**静态。** 创建或截断 staged sidecar 后、写全标头前进程终止，可能留下零长度或短前缀。下一次 open 首先要求该临时文件带完整标头；新下载和续传都会得到 AlreadyExists。现有用例覆盖完整、可识别的中断文件，未覆盖这个崩溃窗口。建议让每次暂存使用新排他名称，恢复使用私有目录中已提交的 meta；无需删除无法识别的公共文件。验收在创建后、部分写入后和 rename 前分别中断，再次下载应能自动恢复且保全无关文件。

### F13 · medium · 未绑定 Agent Process 的补偿仍会在临时错误后永久丢失（Q19）

`crates/ma2a-runtime/src/agents/instance.rs:108`、`:121`、`:128`。**静态。** 已修复“成功读到定义不存在”分支。若 stop/undefine 已移除定义，而绑定之后的补偿查询恰遇 Store 错误，release_unbound 会直接返回；若 terminate 失败，其结果被丢弃。定义已不存在，Agent 维护下一轮无法再由该定义找到新启动的 Process。建议在确认终止或确认被当前代绑定前，保留待核对的 ProcessId；只重试瞬态错误。验收在原交错中分别令第一次查询和第一次终止失败，恢复后应无额外运行进程。

## A2A、托管调用与本地集合

### F14 · high · Windows 自定义 A2A 凭据路径仍继承父目录权限（Q22）

`crates/ma2a-app/src/a2a.rs:43`、`:45`、`:51`、`:54`、`:98`，`docs/protocol/a2a-adapter.md:18`。**静态；未在原生 Windows 执行。** 当操作者选用允许其他本机账户读取新文件的自定义目录时，私有创建模式仅在 Unix 编译；Windows 在这个目录创建 staging 并重命名，没有设置或核验文件 DACL。文档提醒已经补齐；自定义路径中的凭据仍可能被同机其他账户读取，权限等同适配器操作者。较小修法是先核验自定义父目录与成品满足私有访问条件，否则拒绝启动；完整方案在创建 staging 时直接指定受保护的 DACL。验收应在原生 Windows 分别使用受保护目录和共享继承目录，检查 staging 和最终文件从创建到发布的有效权限。

### F15 · medium · 两个 A2A 监听器仍共享最终凭据名，停止时的读取与删除也未原子化（Q23）

`crates/ma2a-app/src/a2a.rs:42`、`:54`、`:59`、`:98`、`:124`。**静态。** 两个进程在不同端口使用同一状态目录，第二次发布仍替换第一个监听器就绪信息中的默认文件。两个服务同时运行时，新客户端无法据第一个监听器给出的路径取得对应凭据。另一个交错是先读得本实例凭据，另一实例发布，然后前者按路径删除新文件。新增测试只覆盖另一实例先完成写入再开始清理。建议按监听实例分配私有文件；需维持固定路径时，为该路径持有覆盖整个监听生命周期的排他租约。验收应让两个监听器重叠启动并分别停止，各自的就绪信息在整个可服务期间都能使用；若采用互斥设计，第二个应在发布前收到明确冲突。

### F16 · medium · 托管调用的 manifest 与占用名额的 link 可属于不同注册代际（Q24）

`crates/ma2a-runtime/src/hosting/call.rs:114`、`:130`、`:140`、`:143`，`crates/ma2a-runtime/src/hosting/link.rs:41`。**静态。** 调用任务取得旧注册快照后暂停，服务管理侧可停止、替换并重新 attach；调用继续时按旧 manifest 检查操作与并发上限，按名字选中新 link 并占槽。即便服务自行拒绝旧操作，Runtime 的新并发上限仍可能失效。最小改动可复用 attach 已有的生命周期锁，从读注册起持有至通知入队和占槽结束，在转发或等待服务 I/O 前释放。验收在注册读取点设置同步屏障，替换为操作集合更小、max_calls 更低的注册，确认同一准入决定始终使用同一代。

### F17 · medium · exposure 的字节限制只保护新写入，升级前的合法集合仍无法列出（Q25）

`crates/ma2a-runtime/src/ipc/server/execute/services.rs:64`、`:79`、`:127`、`:143`，`crates/ma2a-runtime/src/api/responses.rs:283`、`:308`，`web/src/api/services.ts:204`。**静态。** 新增或替换时已按最宽响应信封预检。旧版本允许 128 个不同根名称指向同一条 512 字节的有效目录路径；路径总长已达 65,536 字节，加上 JSON 字段后超过普通 IPC 响应上限。使用这样一份数据库升级，list 仍一次返回全部记录；CLI 无法列出，Web 聚合查询也会失败。用户知道旧名称时可逐个解除暴露。同一 IPC 服务的常规请求受 `crates/ma2a-runtime/src/ipc/server/dispatch.rs:128` 的锁串行化；另一个写入入口 `crates/ma2a-store/src/service_exposures.rs:115` 本身不检查序列化大小。建议为读取增加按字节预算分页，使已有数据库可管理。验收用旧版创建超限集合，升级后列全每项并能逐个编辑，同时保留新写入的边界测试。

## 运维、SDK、Web 与网络观测

### F18 · high · 普通 provenance 文件仍会被 E2E 脚本当作递归删除目录的授权（Q27）

`scripts/run-e2e.sh:18`、`:21`、`:23`。**静态，未执行删除。** 操作者误把输出设成一个存有其他工作的目录，只要其中恰有普通文件 `provenance.txt`，脚本就会删除整棵目录树。即使目录确实产生过 E2E 证据，后来加入的非证据文件也被一并删除。修复能挡住无标记、无同名文件的非空目录。建议每次在输出目录下排他创建唯一运行子目录，已有数据只读；需要轮转时只清理可逐项证明为脚本生成的内容。验收使用隔离夹具，覆盖普通同名文件和已有证据夹杂其他文件两种场景，结束时核对原始文件内容。

### F19 · medium · 第三方服务 SDK 会无限重试不兼容的本地 IPC 版本

`crates/ma2a-service/src/host.rs:98`、`:109`、`:115`、`:131`，`crates/ma2a-service/src/lib.rs:78`，`crates/ma2a-runtime/src/ipc/client/stream.rs:80`、`crates/ma2a-runtime/src/ipc/client.rs:122`。**静态。** 第三方服务连接到一个可响应但版本不兼容的 Runtime；握手已识别 VersionMismatch，转换为 SDK 的 Ipc 错误后进入通配重试分支。调用方既收不到版本错误，也无法区分持续不兼容与暂时没有可用 Runtime。建议仅重试明确的临时错误，把版本不兼容连同诊断返回。验收应同时覆盖持续不兼容时及时返回，以及兼容 Runtime 临时重启时自动重连。

### F20 · medium · 已停止的 Web RuntimeController 仍能清除新会话的页面状态

`web/src/runtime-controller.ts:69`、`:73`、`:107`、`:221`，`web/src/app.tsx:84`、`:89`、`:93`。**探针及静态。** 启动时的 snapshot 仍在等待，组件切换会话，旧 controller 已 stop。若旧请求随后失败，catch 仍调用 handleError；普通错误会覆盖新页面的加载状态，旧请求的 401 则调用 onSessionExpired，清除当前 React 会话并导航至登录页。实际 controller 搭配测试传输夹具，在 stop 后交付一次迟到的普通错误，观察到 1 次错误回调；401 的影响根据同一分支及 App 回调核实。建议在共用错误分发入口核验 controller 是否仍有效。验收在新 controller 成功启动后分别完成旧请求的失败和 401，确认界面和新会话均保持有效。

### F21 · low · 网络诊断缓存满时按 EndpointId 排序淘汰，最近观测也会消失

`crates/ma2a-net/src/telemetry.rs:10`、`:121`、`:239`、`:243`。**静态。** 已观测 128 个 peer，其中键值最小的一个刚刚建立成功连接。加入第 129 个不同 peer 时，BTreeMap 的第一个键被当作 oldest；最新的诊断整组消失，而长期未活动但键值大的 peer 仍保留。影响连接诊断历史。建议依据各队列最后一次观测时间选择淘汰对象，并定义相同时间的次序。验收将键序与活动时间反向排列，核对被淘汰的是最久未观测的 peer。

### F22 · low · exact-pin 检查接受一部分 npm 版本范围

`xtask/src/pins.rs:119`、`:225`。**静态。** 有效依赖范围 `1.2.3-beta || ^2.0.0` 在首个连字符处被截断，剩余的 `1.2.3` 满足“三个十进制字段”的判定。更新依赖时，允许浮动到其他版本的声明仍能通过仓库精确锁版本门禁。建议用严格的完整版本解析器消费整个声明。验收覆盖精确预发布版、精确带构建元数据的版本、含预发布版的联合范围以及普通范围。

### F23 · medium · 按目标归档签发的 SBOM 含有工具包及其他平台专用依赖

`scripts/generate-sbom.sh:15`、`:26`、`:68`，`scripts/validate-release-metadata.sh:15`，`.github/workflows/release.yml:98`、`:112`。**探针及静态。** 每个发布目标使用同一份未筛选的 workspace metadata 包集合，直接写入以该归档命名的 SBOM 并提交证明。对当前锁文件实测，输入同时包含 xtask 和 Windows 用的 wmi；二者均不在 Linux ma2a-app 的 normal 依赖树内。验证器只核验若干必有包和归档名，因而不能发现这种错配。使用该文件盘点二进制依赖会得到错误的平台适用范围。建议从发布根包、目标及实际特性解析依赖；若另需记录构建工具，标明其角色。验收以 Linux 和 Windows 两份实际解析树核对相应归档中的依赖及其角色。

### F24 · medium · 合法的分段 push 已提交前缀，却向发端返回无效请求

`crates/ma2a-runtime/src/control_sync.rs:195`，`crates/ma2a-runtime/src/control_sync/pages.rs:57`，`crates/ma2a-runtime/src/control_sync/exchange.rs:43`、`:59`，`crates/ma2a-runtime/src/control_sync/pages/pull.rs:14`。**静态。** 领先端按自己的完整链头生成游标，push 却从第一份 manifest 起受单页预算约束。落后端接受连续前缀并提交，再为发端构造 pull 响应；此时请求游标仍领先于刚提交的局部链头，被判 Invalid。即使所有 manifest 等长并完全连续也会发生。同方向以相同预算再试仍从头发送同一前缀；落后端另行发起反向拉取有机会收敛。建议显式表示“我的链头较落后”，返回实际接受的游标让发端续传，并保留同一代不同哈希的冲突检查。验收以两个已互认成员及超单页的连续等长历史，只由领先端发起多轮同步，每轮正确报告已提交的范围，最终达到相同链头。

### F25 · medium · 第三方服务 SDK 的调用端无法发送协议支持的 control 帧

`docs/protocol/service-v1.md:123` 明确 control 可双向传递；`crates/ma2a-service/src/call.rs:44`、`:113` 支持服务收、发 control。调用端 `crates/ma2a-service/src/caller.rs:41`、`:52`、`:77`、`:95` 的流只公开读取、发送 data、end 和 cancel，写端私有。**静态。** 服务接受一个流式调用后，等待调用者用约定好的 control 更新参数；使用公开 Caller SDK 的客户端无法发出这条消息。建议为 CallStream 增加有相同 16 KiB 上限的 control 写操作。验收用 SDK 两端交换一次控制消息，确认通道及 payload 不变，并覆盖最大长度和超限错误。

### F26 · medium · Unix 在线备份在创建之后才收紧文件权限

`crates/ma2a-store/src/backup.rs:20`、`:21`、`:23`，`crates/ma2a-store/src/permissions.rs:45`，`Cargo.toml:58`、`Cargo.lock:1982`，`docs/storage.md:92`。**静态。** 单个调用将备份写入其他本机账户有目录搜索权限的位置，进程 umask 允许组或其他用户读取。项目使用 bundled SQLite，锁定版本的 Unix VFS 对新普通主数据库默认使用 0644 再应用 umask。备份函数先打开新文件，再按路径改成 0600，最后才复制内容。创建与改权限之间已取得的读句柄在 chmod 后仍有效，因而备份中的进程凭据和环境值可能暴露。即使只有一个备份任务，这个权限窗口也存在；放在仅限当前用户的目录中可消除这个前提。建议从创建时即使用排他、0600 的私有暂存文件，与 F07 的成品发布修复一并实施。验收应在允许目录搜索的父目录中检查文件创建瞬间的 mode 和最终 mode，验证二者都仅限当前用户，同时检查备份仍可恢复。

## 第四轮 28 项逐项复核

“关闭”针对第四轮的具体触发条件。表中列出改变行为的提交；`b1935bb` 和 `8ded423` 是审查记录提交。

| 第四轮 | 当前判断 | 提交与当前证据 |
| --- | --- | --- |
| Q01 | 原信息泄露关闭，跨入会周期的重试语义仍有问题 | `f6e87c3`。重复离场返回裁剪过的历史；重新取得成员资格后见 F01。 |
| Q02 | 原 pull 场景关闭 | `fb168ee`。`crates/ma2a-runtime/src/control_sync/pages/pull.rs:26`、`:37`、`:52` 在 manifest 未齐时推迟其他记录，并按当前成员及 relay 权限过滤。处理表写作 push，实际改动位于构造拉取响应的 pull_page。独立的页内连续性问题见 F05。 |
| Q03 | 签署不可交付历史的原入口关闭，容量耗尽处理仍有缺口 | `1c78806`。`crates/ma2a-store/src/owned_spaces.rs:218`、`:230` 和 `crates/ma2a-store/src/enrollment.rs:238`、`:248` 在提交前检查导出和分页；额度边界及跨请求影响见 F02、F03。 |
| Q04 | 65 个 Space 的准备阶段已修，分片调度和完成判定部分关闭 | `6ecd07c`。64、65、129 三种计数对照见 F04。 |
| Q05 | 已补文档，行为仍开放 | `181b0fd`。新鲜候选仍是显式和周期同步的共同前置条件，见 F06。 |
| Q06 | 开放 | 区间内未改变公开备份 API 的发布顺序，见 F07。 |
| Q07 | 关闭 | `9e16401`。`docs/storage.md:92`、`:99`、`:101` 说明备份保有凭据和可解码环境；`crates/ma2a-store/tests/migration.rs:77` 对照当前列清单。 |
| Q08 | 开放 | 访问掩码和 SID 唯一性谓词未变，见 F08。 |
| Q09 | 关闭 | `6df24a8`。`crates/ma2a-runtime/src/web/runtime_routes/events.rs:62`、`:204` 首帧基线和后续轮询均在异步取 stamp 后复查授权。 |
| Q10 | 关闭 | `1526afc`。`crates/ma2a-runtime/src/web/auth.rs:53`、`:177`、`:208` 在启动时打开专用连接，两个会话检查在阻塞任务内短暂持锁。仍查询数据库，因此可看到另一连接已提交的撤销；登录及凭据变更另开连接不在 SSE 稳态循环。 |
| Q11 | 文档问题关闭 | `712a9d3`。`docs/security.md:49` 与显式非回环绑定及 CLI 帮助一致，区分默认监听和需要额外保护的明文链路。 |
| Q12 | 部分关闭 | `63b0183`。常规区分大小写的文件系统上挡住精确暂存名；大小写折叠卷见 F09。 |
| Q13 | 部分关闭 | `63b0183`。远端精确拼写已受限。恢复时的创建证明、父目录改名及其他文件系统的名字语义见 F09、F10。 |
| Q14 | 部分关闭，并有恢复回归 | `859d823`、`9a76073`。无有效 meta 的非空外来 part 和无标头的外来暂存文件会被拒绝；Unix 仍有按 inode 重开 lease 的重试。剩余归属问题和中断窗口见 F11、F12。 |
| Q15 | 原终态帧拼接问题关闭 | `022e7b0`。`crates/ma2a-services/src/frame_io.rs:189`、`:207`、`:265` 在第一个异步写前标记未完成，整帧成功才清除；后续帧不能接在被取消的半帧后。 |
| Q16 | 文档问题关闭 | `b5fff68`、`96ab5c1`。`docs/protocol/services-v1.md:297` 说明八个位全量替换、保留其他位的写法及清除授权的后果。 |
| Q17 | 原临时 I/O 错误关闭 | `a1c6e6a`。`crates/ma2a-runtime/src/processes/apply.rs:152`、`:173` 与 `crates/ma2a-runtime/src/processes/follow.rs:71` 区分不可读、无文件和坏数据；临时 I/O 失败会再次观察。 |
| Q18 | 原丢失待删除段的问题关闭 | `ffc43da`。`crates/ma2a-services/src/supervisor/log.rs:197`、`:203`、`:208` 在删除成功或文件已不存在后才出队；其余错误保留队首供下次 append 重试。这不提供无后续输出时的定时清扫。 |
| Q19 | 部分关闭 | `8d7cc21`。确认定义已删除后会终止 Process；补偿出错后的责任归属见 F13。 |
| Q20 | 关闭 | `73f8e62`。`crates/ma2a-agent/src/sdk.rs:118`、`:141`、`:270` 同时等待输出失败，输入结束时有界排空待写数据，并传播写入或 flush 错误。 |
| Q21 | 关闭 | `37873de`。`crates/ma2a-app/src/a2a/time.rs:15`、`:61`、`:75` 按年月检查日期并拒绝空的小数部分。 |
| Q22 | 已补文档，Windows 实现仍开放 | `9b37ad3`。自定义路径的继承权限见 F14。 |
| Q23 | 部分关闭 | `9b37ad3`。分进程暂存和发布后依次退出的简单场景已修，最终共享名和检查后删除的交错见 F15。 |
| Q24 | 开放 | 准入与 attach 的生命周期锁范围不同，最小锁定范围及验收见 F16。 |
| Q25 | 新写入预检已修，既有超限状态开放 | `8321d0c`。预检包含替换项和完整信封；读取与升级兼容性见 F17。 |
| Q26 | 原轮询错序关闭 | `7efd26c`。`web/src/screens/use-services.ts:38` 等上一轮 settle 再安排下一轮，卸载时阻止再次调度。F20 涉及另一个 controller 的生命周期。 |
| Q27 | 部分关闭 | `9e70b5a`。非空目录识别仍有误判，见 F18。 |
| Q28 | 原依赖门禁阻断关闭，维护风险按期接受 | `a97e8c5`。`deny.toml:10` 的例外注明原因和 2027-05-20 复查日，本次独立 cargo-deny 检查通过；锁文件中的传递依赖仍在。 |

## 待确认

**第四轮待确认二：控制响应在传输背压下的准入名额回收上界。** `crates/ma2a-net/src/control/handler.rs:125`、`:147` 在持有许可时等待写入，之后等待关闭另有时限；客户端读操作在 `crates/ma2a-net/src/control/protocol.rs:98`、`:115` 有 10 秒时限。锁定的 noq-proto 1.2.0 默认单流接收窗口按源码常量计算为 1,250,000 字节，大于一次普通最大控制响应。仅看到服务端缺少应用层写时限，不能推出受支持连接上会无限占用名额。本轮未测得传输回收的实际最坏时间。验收可用可控的阻塞写端测试超时后许可确实释放，再在支持的 QUIC 配置测定上界；若需要协议层保证，可在整个 write_response 外加一个总时限。第四轮待确认一已转正文 F05。

## 验证与证据

本轮确认 **26 项，critical 0、high 6、medium 17、low 3**。其中 17 项涉及第四轮已编号问题的残留或修复回归，1 项来自上轮待确认，8 项为扩展审查所得。待确认的传输回收上界单列。应优先处理 F02 的单次请求导致 actor 退出、F03 的旧授权无法撤销，以及 F09、F11、F18 的文件完整性或误删除问题。

在仓库根目录可按以下顺序核对：

```sh
cargo build --locked --workspace --bins
CARGO_TERM_COLOR=never cargo run --locked -p xtask -- check
cargo nextest run --workspace --all-features --no-fail-fast --color never --status-level fail --failure-output never
cargo deny check
cargo machete
(cd web && bun run check && bun run test)
python3 .omo/reviews/2026-11-21/run-page-sizing.py
bun .omo/reviews/2026-11-21/controller-stop.ts
python3 .omo/reviews/2026-11-21/sbom-scope.py
```

- **Rust。** 先成功预构建全部二进制。首次完整 xtask 通过其前置检查、格式和 Clippy，在 nextest 阶段退出 100；保留的首次摘要不能定位具体失败用例，未据此归因。相同基线独立复跑全套 Rust 测试：137 个测试二进制，942 通过、0 跳过。两次记录分别见 `.omo/reviews/2026-11-21/workspace-check.txt`、`.omo/reviews/2026-11-21/nextest-repeat.txt`。
- **依赖与 Web。** 独立 cargo-deny 的 advisories、bans、licenses、sources 均通过，cargo-machete 未发现未使用依赖。Biome、TypeScript 通过；同一套 150 个前端测试在 Bun 和 Vitest 各通过一次。此次使用 Bun 1.4.0，项目和 CI 固定 1.3.5。摘要见 `.omo/reviews/2026-11-21/dependency-check.txt`、`.omo/reviews/2026-11-21/web-check.txt`。
- **三个离线探针。** 分页应观察到第 47 代放不下、第 48 代被选中，接收链拒绝跳代。Web 生命周期探针应观察到停用后的 1 次错误回调。SBOM 探针应显示 xtask 和 wmi 均进入发布脚本输入，但均未进入 Linux 应用的普通依赖树。结果依次见 `.omo/reviews/2026-11-21/page-sizing.txt`、`.omo/reviews/2026-11-21/controller-stop.txt`、`.omo/reviews/2026-11-21/sbom-scope.txt`。探针中的成员、挂起请求及空页都是测试夹具。

## 覆盖度

“深审”指对列出的功能从入口追到存储、响应或失败边界。整套测试通过仅计为运行验证。基线文档包括 README、架构、安全、限制、存储及对应协议，并对照第四轮报告和处理表；当前工作树没有仓库级 AGENTS.md 文件，本轮采用会话提供的项目规则。

| crate 或模块 | 深度 | 已核实的范围与边界 |
| --- | --- | --- |
| ma2a-core | 深审 | manifest 签名、代际及撤销转换，链导出、入会分页、控制帧的计数与字节上限；其余服务消息抽查。 |
| ma2a-store | 深审 | 两条签名写入路径、迁移敏感列、入会事务、备份发布、ACL、exposure 持久化；未模拟掉电恢复。 |
| ma2a-net | 深审 | 控制限流、连接关闭和客户端超时、地址候选与遥测淘汰；relay、地址查询接口抽查，QUIC 持续背压上界待确认。 |
| ma2a-runtime 的成员及控制同步 | 深审 | 离场、重新入会、actor 错误传播、分片排队与等待者、页预算、暂存与提交先后。 |
| ma2a-runtime 的本地 IPC、Web 及托管调用 | 深审 | 请求串行化、exposure 编码、SSE 首帧和稳态会话查询、hosted 注册到通知和占槽；其余 HTTP 路由抽查。 |
| ma2a-services | 深审 | 暂存路径、下载归属和租约、上传恢复、帧写取消、supervisor 日志保留；Exec、Shell、Proxy 主要复核修复共有的帧边界。 |
| 持久 Process 与 Agent Runtime | 深审 | 断连时的状态恢复、未绑定进程的补偿和停止代际；任务队列、外部引擎协议其余部分抽查。 |
| ma2a-agent | 抽查 | SDK 输出错误、EOF 排空及对应回归用例；其他 SDK 协议入口未逐分支检查。 |
| ma2a-service SDK | 深审 | host 附着及重连、版本错误传播、调用端和服务端的流接口与协议对照；独立第三方服务部署未执行。 |
| ma2a-app CLI、A2A、E2E | 抽查 | 深入核实 A2A 时间解析、凭据发布和退出清理；CLI 授权语义与 E2E 入口抽查，Linux workspace 测试包含 E2E。 |
| web/src | 深审 | RuntimeController 的启动、取消和回调归属，React 会话状态、Services 轮询、列表聚合；其余页面抽查，未做浏览器全链路时序测试。 |
| xtask | 抽查 | check 执行顺序、依赖精确锁定谓词及发布检查入口。 |
| scripts、.github 工作流 | 抽查 | 深入核对 E2E 输出目录和生成、验证、证明 SBOM 的发布链；核对平台矩阵，未触发远端 CI 或真实发布。 |
| docs 与实现 | 深审 | 对照 enrollment、Space、control、远程服务、A2A、Web 安全、存储及限制文档；其他叙述按相关性抽查。 |
| 原生 Windows/macOS 行为、真实发布与证明 | 未审（运行时） | 本轮在 Linux 运行。相关平台分支及工作流仅按源码审查；F08、F09、F14 指明了生效条件和原生验收。 |

