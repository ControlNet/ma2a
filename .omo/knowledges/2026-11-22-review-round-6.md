# MA2A 第六轮代码审查

基线为 `1d96672`，本地 master 与 origin/master 指向同一提交。检查了 `8ded423..1d96672` 的 22 个提交，以第五轮报告、处理表和协议说明为对照。下文 F 编号属于第六轮；括号内的“上轮”指第五轮。已执行验证限于定向用例和隔离的离线夹具。平台和并发条件只经源码核实时逐项标明。

## Space 成员、回执与请求失败隔离

### F01 · medium · 两次离场后，第一次请求取回的是第二次离场回执（上轮 F01）

**位置与静态证据。** `crates/ma2a-runtime/src/departure_actor.rs:183` 在发起者当前不在成员表时直接调用 `through_removal`。该函数沿链保留最后一次移除（`crates/ma2a-runtime/src/departure/receipt.rs:20`、`:31`）。按请求号读取持久回执在后面的 Store 分支（`crates/ma2a-runtime/src/store/membership.rs:47`）。  
**触发和影响。** 同一成员先后离场、重新入会、再次离场，再重试第一次已成功的请求。库中虽有两张回执，网络入口却返回第二次移除代的历史。这次重试不额外提交移除，但同一请求的结果改变，违反协议的按请求重放约定。迁移脚本只创建空表（`crates/ma2a-store/migrations/0011_departure_receipts.sql:6`）；升级前的请求也没有可用记录，重新入会后再次收到旧请求仍有误移除风险。  
**建议与验收。** 不论当前是否在会，先查询准确回执；无回执时再走明确的兼容策略。对旧记录无法恢复的 RequestId，应说明迁移边界或引入绑定入会代际的请求。在真实 actor 入口连续执行两个资格周期，核对不同请求各自返回原先的代与哈希，revision 不动；另验 schema 10 升级场景。

### F02 · high · 把新成员换入同一条撤销更新即可破坏可撤销容量不变量（上轮 F03）

**位置与静态证据。** `crates/ma2a-store/src/space_capacity.rs:46` 用非 owner 的**人数**判断是否有新增成员，`:68` 对这种“移除”免检剩余代数和撤销表容量；`crates/ma2a-runtime/src/actor/handle.rs:231` 接受完整的 owner 更新，`crates/ma2a-core/src/space_chain.rs:279` 校验移除已登记及旧撤销的保留，但允许同代加入不同成员。  
**触发和影响。** 在第 239 代保有 16 位非 owner 成员是合法状态。下一代批量移除一人并加入一个新 Endpoint，人数不变且有新撤销，按特例放行。此时有 16 位成员、仅剩 15 代，再逐人移除就会有一人留在旧 Space。单人 CLI 移除和普通邀请已受保护，问题发生于公开的 owner 更新接口。  
**建议与验收。** 比较成员 ID 集合；仅对新集合为旧集合真子集的收缩更新给予旧历史兼容，其余更新都检查结果不变量。以相同人数、成员不同的替换和真正批量移除作为正反用例，验证拒绝时链头与 revision 均未改变。对升级时已经达到 255 代或已用满撤销表的历史，仍需独立的迁移或封存办法。

### F03 · high · 成员名额已满时，正常邀请兑换仍会让 owner 的 actor 退出（上轮 F02 的相邻分支）

**位置与静态证据。** `crates/ma2a-store/src/enrollment.rs:228`、`:239`、`:240` 在加入新成员后先做历史容量检查。默认上限 64 人含 owner，新候选令总人数达 65，但此时非 owner 只有 64；无既有撤销、历史较短时能通过历史容量检查，下一步在 `crates/ma2a-core/src/manifest.rs:45` 命中协议输入错误。对于更低的 Space 策略上限，`crates/ma2a-core/src/space_chain.rs:138` 也可能拒绝。  
**触发和影响。** 创建的有效邀请在兑换前已被其他入会用尽成员名额。该请求应被拒且不消耗邀请；当前错误经 `crates/ma2a-runtime/src/enrollment/owner.rs:58`、`crates/ma2a-runtime/src/error.rs:88`、`crates/ma2a-runtime/src/actor/run.rs:133` 退出主循环，其他 Space 同受影响。此次新增的业务错误分类确实处理了 SpaceHistoryFull。  
**建议与验收。** 在签名前检查 Space 策略允许的总人数，映射为请求范围的业务拒绝。对低上限及默认上限，验证最后一个名额成功、下一张邀请收到拒绝、邀请保持未消费、同一 actor 随后仍能处理其他请求。

### F04 · high · 非 authority 的普通成员收到离场请求时，角色错误会终止自身 actor

**位置与静态证据。** 每个 Endpoint 安装 enrollment handler（`crates/ma2a-net/src/endpoint.rs:160`）。`crates/ma2a-runtime/src/departure_actor.rs:177`、`:180` 核对链及离场者身份，未先确认本机是该 Space 的 authority；普通副本没有 authority key 引用，`crates/ma2a-store/src/owned_spaces.rs:213` 返回 SpaceAuthorityUnavailable。此错误不在 `crates/ma2a-runtime/src/error.rs:88` 的请求拒绝分类内。  
**触发和影响。** 已认证成员的离场请求送到了持有同一 Space 链但不负责签名的副本。标准 CLI 正常会选 owner；接收方仍必须独立处理错误的目的地。当前请求不能签名，却会通过 `crates/ma2a-runtime/src/departure_actor.rs:148`、`crates/ma2a-runtime/src/actor/run.rs:133` 中止该副本的其他工作。  
**建议与验收。** 在回执查询和签名前验证本机角色，把“本机不是此 Space 的 authority”作为无副作用的协议拒绝，并继续区分真正的密钥损坏。用普通成员的本地 handler 夹具断言拒绝、revision 不变，下一条消息仍有响应。

## 控制同步、备份与下载

### F05 · medium · 当所有 peer 都只有部分分片成功，全局同步仍报告成功（上轮 F04）

`crates/ma2a-runtime/src/control_sync/scheduler.rs:75`、`:81`、`:82` 与 `crates/ma2a-runtime/src/control_actor.rs:109`、`:133`、`:159`。**静态；每 peer 限流的定向用例已通过。** 某个 peer 的一片成功、一片失败，且本轮没有完整成功的其他 peer。失败集合正确排除了该 peer，但一片成功就设置 latest，因此整轮返回 Ok；全局等待者获得成功的 revision，诊断历史甚至显示 empty。对指定 peer 的等待者已能正确报错。建议区别“未选中 peer”和“选中但未完成任何 peer”，保留已提交分片的变更用于更新投影。验收用相同的分片结果对照全局等待者、指定 peer 等待者和历史，并保留真正空轮的原行为。

### F06 · medium · 领先端重复主动同步仍只能发送同一前缀（上轮 F24）

`crates/ma2a-runtime/src/control_sync.rs:186`、`:196`，`crates/ma2a-runtime/src/control_sync/pages.rs:56`，`crates/ma2a-runtime/src/control_sync/pages/pull.rs:14`。**静态；已执行领先游标的定向测试。** 修复消除了合法请求被拒为 Invalid 的误报，同代哈希分叉仍会被拒。落后端现在返回空页、more 为 false，未告知已接受到哪一代；领先端按自己的链头生成游标，下一轮仍从第一份 manifest 推送。只要历史超过单页且预算未显著变化，同方向显式重试不会推进余下历史。如果落后方缺少领先方的新鲜地址，也无法按文档所期望的方式发起反向拉取。建议返回已接受的游标或安排可确认完成的反向同步；在语义要求进展时向操作者区分传输成功与仍在追赶。验收只让领先方发起两轮以上，检查每轮的已确认代际直至同头；单独保留分叉拒绝用例。

### F07 · medium · 备份的无覆盖发布在硬链接失败时退化为可覆盖重命名（上轮 F07）

`crates/ma2a-store/src/backup.rs:89`、`:98`、`:100`，`docs/storage.md:93`。**静态；本机支持硬链接的并发备份测试已通过。** 两个备份对同一目标发布，两次 hard_link 均因文件系统不支持而失败；两方都在任一 rename 前看到目标不存在。允许替换的文件系统会让两次 rename 均成功，后一份覆盖前一份。代码还会对 AlreadyExists 以外的所有硬链接错误尝试这一回退。建议使用平台原子的 no-replace rename；无法提供该语义时明确拒绝发布。验收在文件操作夹具中强制硬链接不支持，交错两次发布，核对只有一个成功且第一份内容保留，同时覆盖权限错误直接返回。

### F08 · low · 新备份暂存名会令合法的长目标文件名失效

`crates/ma2a-store/src/backup.rs:29`、`:69`、`:77`。**静态。** 暂存 basename 含完整目标名、固定字串和 32 个十六进制字符。在每个分量允许 255 字节的卷上，220 个 ASCII 字节的成品名合法，但生成的暂存分量超过 255 字节，单写者也无法备份。建议使用与目标名长度无关的短随机暂存名。验收选择接近卷分量上限的目标名，备份成功、可重新打开，并仍保持排他发布。

### F09 · high · 下载把预先存在的目录直接当作私有暂存区（上轮 F11）

`crates/ma2a-services/src/client/resume/place.rs:45`、`:53`、`:67`，`crates/ma2a-services/src/client/resume.rs:55`、`:72`、`:114`，`docs/protocol/services-v1.md:280`。**探针及静态。** 下载父目录已存在可供其他账户遍历的同名普通目录。代码只确认它是目录；0700 仅用于首次创建。Linux 探针在预建 0755 目录上调用生产 prepare，成功返回，mode 仍为 0755。随后以普通创建权限写 part、meta；在允许组或其他用户读取的 umask、父目录可遍历时，未发布的内容和来源信息可能提前暴露。一个原本属于操作者的同名空目录还会在成功下载后被 tidy 移除。建议复用前核验所有者、权限和归属，并在已验证的目录句柄下创建文件；无法证明归属时保留原目录。验收覆盖已有 0755 目录、其他所有者的目录和本进程创建的 0700 目录，分别检查访问权及生命周期。

### F10 · medium · 区分大小写的卷上，不同下载目标会互相清除续传进度（上轮 F11 回归）

`crates/ma2a-services/src/client/resume/place.rs:25`、`:29`，`crates/ma2a-services/src/client/resume.rs:53`、`:62`、`:72`。**探针及静态。** Linux 探针确认 `report` 与 `REPORT` 可以容纳不同文件，同时生产函数为二者返回相同的 part。若前一个下载中断，另一个不同目标按普通下载启动，就会截断前者的 part 并移除其 meta；两者同时下载也会发生不必要的租约冲突。建议使目的身份匹配卷的名称等价规则；在无法精确识别时至少验证 sidecar 中完整目的身份，避免清除另一目的地的进度。验收覆盖区分大小写卷上两个独立续传，以及不区分大小写卷上同一目的地仍只有一个写者。

### F11 · medium · 下载目标本身名为辅助目录时，会被自己的初始化占用（上轮 F11 回归）

`crates/ma2a-services/src/client/resume/place.rs:15`、`:35`，`crates/ma2a-services/src/client/resume.rs:51`、`:156`。**探针及静态。** 操作者选择在新目录下载一个名为 `.ma2a-downloads` 的普通文件。探针确认，原本不存在的目标经过生产 prepare 后变成目录；收到文件尾时无法将已验证的普通文件发布到该目录，即使允许替换也不能成功。建议选择与目标互不冲突的存储位置；若产品决定保留此名，至少在创建目录或接收内容前明确拒绝并记入 CLI 约束。验收使用空父目录，对该名称分别测试普通和覆盖模式，检查结果与磁盘状态。

### F12 · high · 下载到 Files 可写导出根时，远端操作仍能改变待发布的本地文件

`crates/ma2a-services/src/files/resolve.rs:36`、`:53`，`crates/ma2a-services/src/client/resume/place.rs:35`，`crates/ma2a-services/src/client/resume.rs:123`、`:152`、`:157`、`:159`。**静态。** 管理员将下载目录同时用作可写 Files 导出根。该服务与下载进程以同一用户运行，0700 对此无隔离作用；Files 保留上传和 write 的暂存名，却允许寻址新下载目录。下载校验流中内容，发布按名字重新找到 part（`crates/ma2a-services/src/client/transfer.rs:61`、`crates/ma2a-app/src/commands/remote/transfer.rs:49`）。并行的目录操作可以令成功发布的对象与通过哈希校验的对象不同。所需权限限定为该根已授权的写者。建议将下载状态放在 Files 无法寻址的命名空间，或根据被验证的句柄发布；明确处理导出根正好指向辅助目录的配置。验收用正常并行文件操作核对每个成功回执与最终摘要，覆盖目录位于导出根内及根直接指向该目录的布局。

## 推迟项、Process、A2A 与运维

### F13 · medium · 所有对端地址同时过期后，两成员 Space 仍不能自行恢复控制同步（上轮 F06）

`crates/ma2a-runtime/src/control_sync.rs:145`、`:156`，`docs/limitations.md:48`。**静态。** 两端均离线超过所有记录的有效期，无第三方引导；显式指定 peer 也必须先通过新鲜地址筛选。该推迟理由在安全边界上成立：没有依据继续信任过期的 relay 授权。但故障会持续到出现新的引导来源。建议提供显式导入当前成员新鲜签名地址的操作，或在同步结果中指明无可用候选。验收覆盖离线恢复，以及过期、非成员和错 Space 的记录仍被拒绝。

### F14 · low · Windows 存储 ACL 谓词仍把部分权限当作完整权限（上轮 F08）

`crates/ma2a-store/src/permissions.rs:131`、`:134`，`.github/workflows/ci.yml:16`。**静态，未运行原生 Windows。** 两条 Allow 规则只需属于白名单 SID、与 FullControl 掩码交集非零即可通过；缺少完整权限或重复同一个 SID 都可能误报合规。缺少本机 Windows 解释了验证限制，不能作为谓词正确的证据；仓库已有 Windows runner。建议逐 SID 检查必需权限位和覆盖范围。在原生 runner 运行只针对该函数的受保护 DACL 用例，覆盖标准、部分、重复及缺失四种状态。

### F15 · medium · 上传恢复按旧路径清扫，父目录改名后会丢失真实暂存文件（上轮 F10）

`crates/ma2a-services/src/transfer/upload.rs:72`、`:76`、`:79`，`crates/ma2a-runtime/src/store/services.rs:162`、`:168`、`:172`。**静态。** 上传正常创建暂存文件后父目录改名，随后 Runtime 中断。重启时原路径不存在，清扫把 NotFound 当作完成并删除日志，移到新路径的未完成文件及已占空间永久失去清理记录。这条路径无需任何人猜中 128 位名称。若原路径已有同名的其他对象，还存在上轮的误删条件。建议使用 Runtime 控制的稳定暂存目录，保留原子发布所需的同卷属性；若仍采用日志，恢复时必须能确定对象归属。验收覆盖改名、原处为空和原处已换对象，并区分“已清理”和“无法定位”。

### F16 · medium · 未绑定 Agent Process 的清理重试耗尽后仍无人接手（上轮 F13）

`crates/ma2a-runtime/src/agents/instance.rs:19`、`:111`、`:126`、`:143`、`:150`。**静态。** stop/undefine 与启动交错，绑定返回零行，随后查询或终止连续四次失败，函数只记日志；退避的额外睡眠合计 600 ms。服务稍后恢复时定义已不存在，维护任务不会再从该定义找到 Process。另外，绑定本身返回 Err 时连这四次清理都不会进入。建议将待核对的 ProcessId 持久化，明确确认绑定或终止后才消除，并在重启时接续。验收分别在绑定失败、第四次补偿后恢复、补偿中途重启三个位置注入暂态错误；保全已被并发任务合法绑定的进程。

### F17 · high · Windows 自定义 A2A 凭据仍继承共享目录权限（上轮 F14）

`crates/ma2a-app/src/a2a.rs:43`、`:45`、`:51`、`:54`、`:128`，`docs/protocol/a2a-adapter.md:18`。**静态，未运行原生 Windows。** 使用允许其他本机账户读取新文件的自定义父目录时，普通创建没有指定私有 Windows DACL，也未核验成品；其他账户可能取得有效的操作者凭据。生命周期锁处理实例协调，不能保护凭据内容。文档已说明自定义目录须私有，适用前提较窄，平台延期仍留下可配置的暴露。建议从创建起使用私有 DACL，或在确认不了目录有效权限时拒绝启动。验收检查共享父目录中的暂存及最终文件，同时保留私有自定义路径的正常功能。

### F18 · medium · A2A 自定义凭据名可以占用另一监听器的锁文件名（上轮 F15）

`crates/ma2a-app/src/a2a.rs:54`、`:67`、`:75`、`:122`。**静态，针对允许重命名已打开文件的 Unix 文件系统。** 第一实例使用普通凭据名，第二实例合法选择“第一凭据名加 .lock”作为自己的凭据名。两者申请不同的锁，第二个发布凭据时却替换第一实例锁路径的目录项；第一实例继续持有旧 inode，后来再申请第一凭据名的实例可锁到新 inode。结果是两个活跃实例再次共用并覆盖一个最终凭据名。建议把锁放到凭据发布不能占用的专用命名空间，并拒绝自定义路径与该空间重叠。验收组合两个有后缀关系的合法路径，确认第二个监听器不会使第一个失去排他性，停止任一实例也不会删除另一实例的凭据。

### F19 · medium · 超大 exposure 集合的新错误可读，但用户仍不能发现要移除的条目（上轮 F17）

`crates/ma2a-runtime/src/ipc/server/execute/exposure_bound.rs:40`、`:44`，`crates/ma2a-runtime/src/ipc/server/execute/services.rs:114`、`:143`，`crates/ma2a-store/src/service_exposures.rs:115`、`:152`。**静态及定向测试。** 读取现在返回 conflict 和操作建议，已知名称时可解除暴露；不知道旧名称的操作者仍无法通过列表获取选择器。当前公开 Store 写入接口没有 IPC 的序列化大小约束，故这种状态不限于旧开发版数据库。精确版本协议确实增加分页改动成本。较小方案是增设本地只读恢复入口，仅按有界页列出 Space、类型和可用于删除的选择器，并把持久层约束覆盖到公开写入入口。验收从超限存量库只凭公开工具找到并解除一项，再检查其余条目可完整盘点。

### F20 · medium · E2E 证据目录仍会删除不属于脚本的常规文件（上轮 F18）

`scripts/run-e2e.sh:21`、`:23`、`:29`、`:31`。**仅静态，未执行该脚本。** 目录须带脚本标记，这已修复“普通 provenance 文件便获准递归删除”的旧问题。但 glob 中 `[0-9]*` 是“一位数字后接任意字符串”；操作者放在证据目录里的 `run-1-notes.log` 也被当作脚本产物并删除。即便修严模式，检查和删除分两次遍历，第二次无条件删除当时所有普通文件，仍会误删两次遍历间新加入的文件。建议为每次执行创建独立的新子目录。验收用隔离夹具同时检查近似文件名与检查后新出现的文件，两者内容均应保留。

### F21 · medium · 手动触发发布工作流也会给不分平台的 SBOM 签发证明（上轮 F23）

`.github/workflows/release.yml:6`、`:53`、`:98`、`:111`、`:155`，`scripts/generate-sbom.sh:15`、`:26`。**静态。** Release 工作流除标签外支持手动触发。build 阶段直接以整个 workspace 的 cargo metadata 生成每个归档的包表并运行证明步骤；只有最后的 publish 作业受标签条件限制。各目标仍会纳入 xtask 等工具和其他平台专用依赖。审查时本地没有标签，但“未发版”不能保证上述流程不会运行。建议按发布根包、目标及实际特性计算依赖并区分构建工具；若决定推迟，应在所有入口为证明步骤设置相同的发布前置条件。验收离线比较 Linux 与 Windows 的应用依赖闭包，再检查手动触发路径采用同一验证结果。

### F22 · low · E2E 整理测试清单时中断，会阻止原证据目录的下一次运行（上轮 F18 回归）

`scripts/run-e2e.sh:23`、`:25`、`:33`、`:79`、`:82`。**仅静态，未执行该脚本。** 脚本把执行清单暂写为 `run-1.tests.txt.unsorted`。若进程在删除临时清单之前中断，下次运行发现这个脚本自己创建的普通文件不在允许列表内，于是拒绝复用整个证据目录。建议将中间文件放进每轮私有运行目录；短期可显式识别自己的临时清单，保留对其他文件的保护。验收以含标记和此中间文件的中断状态启动下一次盘点，能生成新证据且保留无关文件。

### F23 · medium · Store 允许把没有移除请求人的 manifest 记为该人的离场回执

`crates/ma2a-store/src/owned_spaces.rs:75`、`:228`、`:238`，`crates/ma2a-store/src/spaces.rs:180`，`crates/ma2a-store/src/departure_receipts.rs:32`。**静态，仅限公开的本地 owner/Store 接口。** 调用者对一个合法的标签或授权更新误用 `OwnedSpaceUpdate::answering`，给它关联某个仍在会成员的请求。签名和成员转换本身有效；存储端未核对这个 Endpoint 是否从在会变为撤销，便在同一事务内生成回执。后来正常提交相同离场请求时，命中回执返回仍包含本人的链，接收方拒绝，实际离场也没有执行。正常网络 handler 构造的单人移除符合条件。建议用专门的移除 API 隔离这种组合，或在事务中核验请求人确由本代移除并撤销。验收应拒绝为普通授权变更、错误成员或既有撤销生成回执，确保失败时链头、revision、回执均未改变。

### F24 · high · 备份创建、校验和发布之间没有固定同一个暂存对象

`crates/ma2a-store/src/backup.rs:29`、`:40`、`:43`、`:47`、`:91`，`crates/ma2a-store/src/permissions.rs:13`。**静态，限于其他进程有权替换目录项的备份父目录。** 备份独占创建私有文件后立即关闭句柄，再通过路径设置权限、打开 SQLite、执行完整性检查，最后按路径发布。若目录整理程序在完整性检查后移动了暂存文件并在原位置放入其他文件，发布步骤仍可能成功，最终备份却不是已校验的数据库。随机命名和 0600 保护新文件的创建及内容访问，不能固定共享可写目录中名字所指的对象。建议让整个暂存生命周期位于受保护、无法被替换的同卷目录，持有目录句柄并从已验证的对象发布；不满足目录权限前提时在创建备份前拒绝。验收在受控文件系统夹具中切换暂存目录项，要求成功结果总与通过 integrity_check 的内容一致，而且清理不会移除其他对象。

## 本轮验证与覆盖

从仓库根目录可单独复核下列命令。Rust 驱动逐次运行一个有名称过滤条件的用例，且检查每次恰好执行 1 项；它不启动 workspace 套件或 E2E 脚本。

```sh
python3 .omo/reviews/2026-11-22/focused.py
(cd web && bun test src/runtime-controller.test.ts -t 'a stopped controller reports nothing')
python3 .omo/reviews/2026-11-22/run-place.py
```

- **已执行。** 8 个定向 Rust 用例均通过，覆盖离场回执事务、硬链接备份竞争、恢复短 sidecar、SDK control 消息、领先游标响应、分片并发上限、同版本 busy 握手分类、超限 exposure 的补救提示。Web 旧 controller 迟到错误的定向用例 1 项通过。摘要在 `.omo/reviews/2026-11-22/focused.txt` 和 `.omo/reviews/2026-11-22/web.txt`。
- **离线探针。** `.omo/reviews/2026-11-22/download-place.rs` 引入该基线的生产路径模块，在 Linux 的独立临时目录检查三个普通目的名和权限场景。输出为不同成品名映射相同 part、预建目录被接受且仍为 755、空闲目的名被创建为目录。它使用仓库现有 debug 依赖；结果在 `.omo/reviews/2026-11-22/download-place.txt`。未连接运行中的 daemon。
- **覆盖范围。** 深审本次改动中的 owner 签名和 SQLite 落库、入会及离场 actor、控制组帧与等待者、备份发布、Files/Transfer 辅助文件、Agent 补偿、A2A 凭据锁及证据目录清理。对 SDK、Web、遥测、xtask、hosted 准入和发布链按修复点及相邻调用者抽查。未执行 E2E 脚本、workspace 全套测试、原生 Windows/macOS、远程工作流或真实发布。第五轮处理表中列出的全量结果属于上一轮执行记录。

## 第五轮 F01–F26 逐项对照

本表的“关闭”限于第五轮提出的触发条件。正文共 **24 项：critical 0，high 7，medium 14，low 3**；其中包含复核后仍开放的问题。待确认项不计入。

| 上轮 | 判断 | 当前证据与本轮对应项 |
| --- | --- | --- |
| F01 | 部分关闭 | `5dcdd7e`。schema 11 回执与移除同事务，新版移除后重新入会的简单重试已修。第二次离场后优先选错回执及迁移边界见本轮 F01；公开写入接口的不变量见 F23。 |
| F02 | 原 SpaceHistoryFull 分支关闭；相邻错误仍可退出 actor | `ff98ab0`。`crates/ma2a-runtime/src/actor.rs:95` 正确吸收已识别的请求拒绝。成员政策名额与错误接收角色见 F03、F04。 |
| F03 | 正常单人更新受保护，批量更新部分关闭 | `2edeaa7`。单人邀请、普通变更检查剩余代数和撤销表空位；识别“无新增成员”时只比较人数，见 F02。 |
| F04 | 指定 peer 关闭，全局完成语义部分关闭 | `cd3b396`。`crates/ma2a-runtime/src/control_sync/dial_queue.rs:35`、`:45` 按 peer 限流并记分片失败，指定 peer 不再假成功。全局等待者与诊断见 F05。 |
| F05 | 关闭 | `782a969`。`crates/ma2a-runtime/src/control_sync/pages/builder.rs:69` 在第一条放不下时停止；push 与 pull 都调用该方法，不再跳代。 |
| F06 | 推迟理由仅解释安全取舍 | 接受现有限制无需放宽过期记录，但控制平面的自恢复限制仍成立。见 F13。 |
| F07 | 部分关闭 | `66e40c1`。私有暂存、完整性检查和硬链接发布正确；无硬链接路径仍无原子保证，暂存长名另有回归。见 F07、F08。共享可写父目录的对象归属另见 F24。 |
| F08 | 推迟，问题仍在 | Windows 本机验证条件不足属实，生产谓词仍弱于声明；建议用已有 Windows CI 执行聚焦用例。见 F14。 |
| F09 | 上轮的 ASCII 大小写变体关闭 | `caced78`。`crates/ma2a-services/src/files/resolve.rs:43`、`:53` 对每个组件按 ASCII 不区分大小写检查上传和 write 前缀。新的下载辅助目录属于另一命名空间，见 F12。 |
| F10 | 推迟理由遗漏了无需名称碰撞的情况 | 单纯父目录改名即可使真实上传残件脱离日志，即使从未出现第二个同名文件。见 F15。 |
| F11 | 部分关闭，出现新的名字和权限边界 | `5026488`。旧式并列辅助名不再占用，普通后缀相同的成品可共存。预建目录、大小写、目录本名及导出根重叠见 F09–F12。 |
| F12 | 关闭 | `84a38d3`。`crates/ma2a-services/src/client/resume/claim.rs:19`、`:43` 仅对 staged meta 接受标头前缀；已执行相应短写恢复测试。 |
| F13 | 部分关闭 | `569266a`。前三次暂态失败后仍有机会清理，耗尽重试或 bind 本身失败的责任未持久化。见 F16。 |
| F14 | 推迟，文档描述准确，代码问题仍在 | 只有 Windows 自定义目录受此条件影响；锁文件未改变最终凭据 ACL。见 F17。 |
| F15 | 同一普通路径关闭；交叉路径未关闭 | `44e75be`。正常情况下锁贯穿 publish、serve、清理。允许把其他实例的锁选为自己的最终文件，见 F18。 |
| F16 | 关闭 | `238e1b1`。`crates/ma2a-runtime/src/hosting/call.rs:124` 至 `:180` 与 register、stop、attach 使用同一生命周期锁，注册快照、通知和占槽属于同一代，网络转发在释放锁后。 |
| F17 | 部分修复判断成立，历史版本限定不成立 | `63569d9`。已执行 conflict/补救提示用例；列举恢复与当前 Store 写入入口见 F19。 |
| F18 | 部分关闭 | `e94e6bc`。已无整树递归删除，原普通 provenance 场景已修。允许模式、检查后删除与脚本自己的中断文件见 F20、F22。 |
| F19 | 关闭 | `be5fcd0`、`b345a8a`。`crates/ma2a-service/src/lib.rs:87` 区分持久版本错误和暂态错误，`crates/ma2a-runtime/src/ipc/client.rs:123`、`:129` 区分异版与同版繁忙，后一项定向测试通过。 |
| F20 | 关闭 | `4fe5d92`。`web/src/runtime-controller.ts:147`、`:229` 统一错误分发并在 stopped 时抑制回调，停止后迟到错误用例通过。 |
| F21 | 关闭 | `c04d5ff`。`crates/ma2a-net/src/telemetry.rs:250`、`:257` 记录每次观测的顺序，在满缓存中按最后观测序号淘汰。 |
| F22 | 关闭 | `6439b2a`。`xtask/src/semver.rs:5`、`:32` 校验所有预发布与构建标识符并消费整个版本，含联合范围的输入不再只按数字前缀放行。 |
| F23 | 推迟理由只适用于最终发布作业 | 无标签不阻止手动 build、构造 SBOM 和签发证明，见 F21。 |
| F24 | 合法请求被判 Invalid 已修，单向追赶部分关闭 | `f0645cb`。实际测试确认空页和同代不同哈希拒绝；空页不能作为同方向续传游标，见 F06。 |
| F25 | 关闭 | `f0e2949`。`crates/ma2a-service/src/caller.rs:96`、`:113` 使用通用帧边界发送并 flush，定向 duplex 用例核对消息及 16 KiB 边界。 |
| F26 | 原 Unix 创建权限窗口关闭 | `66e40c1`。`crates/ma2a-store/src/backup.rs:80` 在排他创建时直接指定 0600；发布回退、暂存对象身份分别见 F07、F24。 |

## 待确认

- **截断 push 对附带 relay 广告的影响。** `crates/ma2a-runtime/src/control_sync/pages.rs:56`、`:74` 在尚有 manifest 未装入时仍尝试加入本机公告；接收侧按本页实际达到的链头授权（`crates/ma2a-runtime/src/control_sync/pages/staging.rs:92`、`:109`）。代码存在因果顺序差异；本轮没有验证一组实际签名长度、授权变化与预算同时成立的生产页。宜用受控同步夹具确认，尚未计为缺陷。
- **传输背压下控制响应的名额回收时间。** 上轮保留项涉及 `crates/ma2a-net/src/control/handler.rs:125`、`:147`。本次提交改变发端调度，未给接收端整次写操作增加时限。本轮没有执行传输背压试验，无法确定支持配置下的实际回收上界。

