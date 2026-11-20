# MA2A 第四轮代码审查

基线为本地 HEAD 与本地 origin/master 共同指向的 `9914f0e`。开始时无已跟踪文件改动。仓库内没有 AGENTS.md，按本次会话提供的规则执行。对照 README、架构、安全、限制、协议及前三轮 R1–R17、D1–D7 记录。下文位置均为此版本的仓库相对路径。每条均说明证据类型；静态结论不代表跑过对应端到端场景。

## 身份、邀请、成员和控制同步

### Q01 · high · 已撤销成员仍可收到撤销之后的新成员历史

`crates/ma2a-runtime/src/departure_actor.rs:174`、`:185`、`:140`。正常离开或被移除后，只要最新撤销列表里仍有该 Endpoint，离场重试分支就把读取时的**整条最新链**分页返回。若撤销时为第 N 代，之后增加成员或改变授权，回包也包含 N 之后的成员标识、标签和能力变化。该分支还在读取本地 authority 身份之前返回。证据为分支顺序、返回值及分页器对所有 manifest 的遍历；传输层鉴别了原成员身份，内容边界仍不成立。同一根因还影响重新加入：兑现新邀请会清除该 Endpoint 的旧撤销记录（`crates/ma2a-store/src/enrollment.rs:216`），而 authority 解码后丢弃 RequestId（`crates/ma2a-runtime/src/departure_actor.rs:135`）。若在重新加入后由同一已认证 Endpoint 重送同一个旧请求，协议处理器会将新成员资格再撤销一次。默认 CLI 的本地重放缓存可能截住重复调用；此结论限定在 authority 所承诺的协议边界（`docs/protocol/enrollment-v1.md:29`）。建议在 authority 端将离场回执按 Space、Endpoint、RequestId 与成员资格代际持久化；重复请求只返回对应签名链前缀。回归断言应确保撤销后的管理变更不出现在旧离场回执中。

### Q02 · high · 同一页的旧地址可阻止新撤销落库

`crates/ma2a-runtime/src/control_sync/pages.rs:101`、`crates/ma2a-runtime/src/control_sync/pages/staging.rs:92`、`:127`。成员 A 已被撤销，但 A 的地址签名尚未到期。一个落后的成员拉取撤销记录，且其地址游标尚未见过 A 的最新序号。发送端从全 Space 的地址高水位中按序号和时间挑选，未按最新成员资格过滤。接收端先应用 manifest 到临时链，再按新成员集校验地址，整批在提交前被拒绝。这次同步保留旧授权，影响持续到另一条同步路径送达撤销，或该地址过期。SQL 只按 Space 取行（`crates/ma2a-store/src/address_record_queries.rs:151`），地址外键只关联 Space；接收端明确检查成员资格（`crates/ma2a-store/src/address_records.rs:77`）。建议发送端保留高水位，但只传播在页面对应授权视图中有效的记录；以撤销与未过期地址同页的双端用例验收。

### Q03 · medium · 长寿命 Space 可写入无法导出的历史

`crates/ma2a-core/src/space_chain.rs:149`、`:158`、`crates/ma2a-store/src/owned_spaces.rs:225`。普通授权变更可把历史推进第 256 代并持久化；公开链编码只支持 255 个 manifest。即使成员很少，持续修改授权也会到达该状态。第 255 代上再兑现邀请已经会因为要导出第 256 代而回滚（`crates/ma2a-store/src/enrollment.rs:246`）。离场回执另有 32 页、每页 8 条的上限：若离场产生第 257 代，authority 先提交移除，后因分页失败报错（`crates/ma2a-runtime/src/departure_actor.rs:139`、`crates/ma2a-core/src/enrollment_page.rs:33`）。证据为三条真实入口的计数与提交顺序。现有测试也明确执行到第 256 代并断言导出失败（`crates/ma2a-core/tests/manifest_golden.rs:110`、`:119`），该测试在本轮通过。建议统一可表示的代数，在签名提交前验证所有必需回执可编码；为已达到上限的 Space 提供保留撤销能力的检查点或协议升级。

### Q04 · medium · 共享 Space 超过单帧上限时，整轮同步准备失败

`crates/ma2a-runtime/src/control_sync.rs:151`、`:162`、`:183`，`crates/ma2a-core/src/control.rs:153`。两个 Endpoint 共同加入 65 个 Space，且在每个 Space 中互有新鲜地址并进入本轮选择时，调度器为一个 peer 合并 65 个游标。预算函数先构造请求，当场触发单帧 64 个 Space 的限制；最终的 Result 收集使本轮其他正常 peer 的请求也无法交给拨号器。无需超长地址或无效签名。建议按协议计数及编码大小将每个 peer 的工作拆帧，跨轮保留剩余项；检查 64、65 个 Space 及额外健康 peer 的进展。

### Q05 · medium · 双端离线跨过地址有效期后，控制同步无法自行引导

`crates/ma2a-runtime/src/control_sync.rs:139`、`:150`，`crates/ma2a-runtime/src/control_sync/scope.rs:98`。只有两名成员的 Space 中，两端同时离线超过地址记录最长 10 分钟，回来后没有第三方提供新记录。各自重新发布本地地址，但每端都先从新鲜记录产生候选，连显式指定的 peer 也只能在这个集合内选。于是没有一端会拨给对方，地址与后续控制状态无法通过正常周期或显式同步恢复。证据为启动、周期、显式入口共用的候选过滤；结论限定在无新引导来源的控制平面。建议增加经当前成员身份验证的重新引导入口，例如允许操作员导入新的已签名地址，并在显式同步无可用地址时报告原因。

## 持久化、备份和恢复

### Q06 · medium · 两个备份可同时接受同一个目标

`crates/ma2a-store/src/backup.rs:15`、`:20`。两个正常备份任务同时选取尚不存在的同名目标，可以均通过 exists 检查，随后以普通可创建、可读写方式打开同一文件。后执行的在线备份可能覆盖先完成的备份，且完整性检查仍可成功。单次备份在完整性检查前失败也会留下最终文件名，阻止原路径重试。当前仓库调用面是公开 Store API，现有 `crates/ma2a-store/tests/backup.rs:23` 只覆盖单写者成功。建议在同目录的排他、受保护临时文件中完成备份与检查，以原子 no-clobber 操作发布；失败只清理自己拥有的文件。

### Q07 · medium · 备份文档漏列可恢复的 Process 凭据和环境值

`docs/storage.md:77`、`:91`，`crates/ma2a-store/src/processes.rs:191`、`:200`。在有活动 Process 时备份，完整 SQLite 副本会带出 32 字节 supervisor 所有权凭据，以及可解码的启动参数和环境值。终止时从源行清除 secret 不会更新旧备份。Process 协议明确要求持久化所有权凭据（`docs/protocol/process-v1.md:462`）；面向备份操作者的通用声明却排除了可恢复秘密。建议准确列出备份的敏感数据、有效期和访问控制，说明哪些身份私钥需单独备份，以及哪些工作负载文件仍需另外恢复。

### Q08 · low · Windows 存储 ACL 的实际判定弱于声明的精确权限

`crates/ma2a-store/src/permissions.rs:134`，`docs/storage.md:15`。校验只要求两条显式允许规则、各自 SID 在二者白名单内，并且权限与 FullControl 的按位交集非零。即使当前用户和 SYSTEM 分别只拥有部分权利，这个谓词也能通过；二者是否各出现一次也没有校验。因此权限诊断可能误报合规，后续写入或恢复才失败。仅按源码检验谓词，未在 Windows 上执行；此项与旧记录的交叉编译工具链阻塞不同。建议比较必需掩码的完整包含关系并逐个核对预期 SID，在原生 Windows 测试缺权及重复规则。

## 私有 IPC 与 Web 控制台

### Q09 · low · 首个 SSE 重同步事件缺少撤销后的复查

`crates/ma2a-runtime/src/web/runtime_routes/events.rs:54`、`:60`、`:79`。建立订阅时会话通过认证，等待初始 snapshot stamp 时会话被撤销；若返回的 revision 与 since 不同，处理器仍直接发送带最新 revision 的 resync 事件。后续轮询的 `authenticated_stamp` 在读取返回后再次认证（同文件第 209 行），首帧没有这项保护。影响明确限于 revision。建议基线读取也走同一复查函数，并把已有轮询中撤销的回归断言覆盖到首帧。

### Q10 · medium · 正常 SSE 订阅会每秒反复执行全库启动验证

`crates/ma2a-runtime/src/web/auth.rs:173`、`crates/ma2a-runtime/src/web/runtime_routes/events.rs:17`、`:205`、`:211`，`crates/ma2a-store/src/repository.rs:63`、`:69`。一个打开的控制台在稳定、无变更时也每秒做两次会话验证；每次重新打开 Repository，执行 SQLite quick_check、检查密钥引用并遍历所有 Space 重验历史签名（`crates/ma2a-store/src/migrations.rs:112`、`crates/ma2a-store/src/spaces.rs:68`）。工作量随无关的历史增长，8 个订阅的常规轮询就约为每秒 16 次全库打开。这个数字是调用次数，不是吞吐实测。建议重用通过启动校验的专用阻塞连接或 Store actor，保留前后两次轻量会话检查。回归应验证增加历史不会增加每次会话检查的签名验证次数。

### Q11 · medium · 顶层安全承诺与可实际启动的 HTTP 监听范围冲突

`docs/security.md:35`、`:47`，`crates/ma2a-runtime/src/web/lifecycle.rs:133`、`crates/ma2a-runtime/src/web/headers.rs:22`。操作者显式选择非回环地址及固定端口时，生产入口使用普通 HTTP 接受远端连接，登录和会话数据经过该链路。顶层安全指南却承诺只在系统分配的回环端口监听并拒绝远端。默认监听符合预期，详细指南 `docs/web-security.md:3` 也准确区分了两种模式。建议统一公开安全承诺，在 CLI 帮助及安全指南说明非回环模式需要受保护的传输；若只支持本机，则应在实际 bind 之前强制执行。

## Files、Transfer、Exec、Shell 与 Proxy

### Q12 · high · 上传核对的是接收字节，发布的是一个可变目录项

`crates/ma2a-services/src/transfer/upload.rs:126`、`:190`，`crates/ma2a-services/src/files/commit.rs:58`。上传临时文件位于可写导出目录内。其他有该根写权限的正常 Files 操作可在上传期间改变临时目录项。接收器以流中已接收的字节计算承诺；发布时同步并关闭原文件句柄，再对**当时该名字指向的对象**做 rename 或 hard link。两者可以不同，成功回执所报哈希就不再描述发布内容。这是服务端上传路径，独立于 R2 的客户端并发下载。建议使用远端 Files 无法寻址的受保护 staging 目录或匿名文件，确保发布对象就是已验证对象。验收应比较最终文件摘要和成功回执，包括并行目录操作。

### Q13 · medium · 崩溃恢复可能删除未归属于本次上传的文件

`crates/ma2a-services/src/transfer/upload.rs:76`、`:79`，`crates/ma2a-runtime/src/store/services.rs:165`。若目标目录中原本已有同名的 staging 文件，程序先提交路径日志，后排他创建文件。两步之间崩溃时，实际上还没有取得该文件的所有权。重启清扫只判断文件名形态并按路径删除，已有文件就会被移除。父目录改名也会使日志路径与原先持有的目录句柄失去对应，导致清错对象或留下真正的临时文件。建议让受保护的 staging 命名空间只容纳 Runtime 文件，恢复时核对对象身份并保留未决清理结果。此项关注重启后删除的所有权，与 Q12 的成功回执完整性不同。

### Q14 · high · 本地下载可以破坏同目录下无关的现有文件

`crates/ma2a-services/src/client/resume.rs:46`、`:57`、`:99`、`crates/ma2a-services/src/client/resume/lease.rs:50`。若本地目录已有与下载辅助名相同的普通文件，非续传下载会直接删除已有 meta、截断已有 part，或在释放 lease 时删除事先存在的普通 lock 文件。即便最终目标不存在或操作者没有给出覆盖许可，这些副作用仍可发生。两个不同的合法目标名也可能分别占用对方的辅助路径。R2 加的锁能协调同一目的地的下载者，但它不建立对原有旁文件的所有权。建议将续传文件及租约放入私有目录，用目的地身份索引；对旧格式，仅接管能验证归属的数据。验收应预置有真实内容的冲突旁文件，验证成功和失败退出均保留内容。

### Q15 · medium · Exec、Shell 和 Proxy 可把终态帧写进未完成的数据帧

`crates/ma2a-services/src/frame_io.rs:193`，`crates/ma2a-services/src/exec/run.rs:119`，`crates/ma2a-services/src/exec.rs:75`。输出链路暂时背压，数据帧头与部分内容已写出时，Exec deadline 到达会丢弃当前写 future。写入器没有保存剩余字节；超时分支不置 stream_error，之后仍用同一 writer 发 outcome。对端只能按旧帧的声明长度解码，于是可能把终态当作 stdout 的一部分，或在帧中间遇到 EOF，丢失本应明确的超时结果。Shell 在取消 PTY 输出后也会写 outcome（`crates/ma2a-services/src/shell/io.rs:153`、`crates/ma2a-services/src/shell.rs:77`）；Proxy 的空闲或总时限可中断下行写入，随后走同一终态路径（`crates/ma2a-services/src/proxy/relay.rs:81`、`:178`、`crates/ma2a-services/src/proxy.rs:57`）。建议将写端做成可取消恢复的状态机；若必须立即停止而不能补齐已开始的帧，应中止该流并显式返回结果不确定。检查限速的正常输出及超时后再次读取的情形。

### Q16 · low · Phase 2 授权说明会引导操作者意外清除后三项授权

`docs/protocol/services-v1.md:289`，`crates/ma2a-runtime/src/store/grant.rs:117`，`crates/ma2a-app/src/cli/services.rs:15`。文档称一次 grant 恰好设置 Files、Exec、Shell、Proxy 的五个位。当前 CLI 将包括 Process、Agent 和 hosted Service 在内的八个位作为完整集合传入；没有给出的标志为 false，Store 会移除对应能力。已有 Process 等授权的成员按旧说明只调整文件访问时可失去其余授权并触发运行中工作的撤回。建议在所有关联协议中统一说明八位全量替换语义，并用保留其他授权的例子说明操作结果。

**R2 差量复核。** `crates/ma2a-services/src/client/resume.rs:143` 的 hard-link 成功路径可以原子防止覆盖；第 146–149 行的回退仍在检查后普通 rename，在支持覆盖式 rename 且 hard link 不可用的文件系统上无法兑现 no-clobber。协议末段已写出回退方法，前三轮已记录这种竞态；本轮不把它计为新增问题。改用系统的 no-replace 原语或在无法保证时返回错误。

## 持久 Process 与 supervisor

### Q17 · medium · 临时状态文件读错会被永久固化为 Process 丢失

`crates/ma2a-runtime/src/processes/apply.rs:125`、`:137`，`crates/ma2a-services/src/supervisor/status.rs:152` 的 `read`。一个已经正常结束的 Process 有完整的终态文件，supervisor 也已退出；若重连时读取该文件暂时失败，`.ok().flatten()` 会把 I/O 错误与文件确实不存在合并，随后将 Starting/Running 记录提交为不可逆的 Lost。即使文件恢复可读，后台只会重新跟随非终态行，原本可证实的退出码丢失。建议保留 `Ok(None)`、损坏数据、可重试 I/O 错误的区别，仅在证据允许时持久化终态；测试一次读失败后恢复。

### Q18 · medium · 删除旧日志段失败会使保留窗口与磁盘永久分离

`crates/ma2a-services/src/supervisor/log.rs:197`、`:200`，`crates/ma2a-services/src/supervisor/host.rs:88`。日志达到保留上限后，队列先弹出最老的段，再删除文件。删除因暂时的文件占用或文件系统错误而失败时，这个段已经不在队列里，之后追加也不会重试。读者从目录枚举段，仍可看到本应淘汰的内容；内存窗口却跳过该段，磁盘占用也可超限。宿主忽略 append 错误。建议删除成功或确认文件已不存在后再移出队列，对其他错误记录可重试清理，并规定日志写入失败的可观察结果。

## Agent、A2A 和托管服务

### Q19 · high · 停用并移除 Agent 时，尚未绑定的后台 Process 可继续运行

`crates/ma2a-runtime/src/agents/instance.rs:93`、`:109`，`crates/ma2a-runtime/src/agents/operator.rs:119`，`crates/ma2a-store/src/agents.rs:151`。维护任务正在为已启用 Agent 启动持久 Process，操作员随后执行 stop 和 undefine。若 stop 提交时实例行尚无 process_id，它没有可终止的 Process；undefine 可删除已停用的定义。启动返回后，带 generation 的绑定更新正确地影响零行，但补偿清理发现定义已不存在便直接返回。该本地 Process 遂脱离 Agent 生命周期，可能继续运行并占用名额。这与旧 R7 的同名任务代际混淆不同。建议将“成功查询且定义不存在”视为应终止，把查询错误纳入可重试清理，并以 ProcessId 记录未完成的补偿；验收以真实后台启动和操作员停止的交错为准。

### Q20 · medium · Agent SDK 不传播 stdout 写入失败

`crates/ma2a-agent/src/sdk.rs:105`、`:112`、`:131`、`:232`。SDK 的独立 writer 在 write 或 flush 失败时返回错误，主循环只等待输入。输出管道关闭而输入仍打开时，serve_on 不会返回写错误；之后即使输入正常 EOF，也会中止 writer 并返回成功。上层据此无法区分正常结束与已失去 ACK、终态或进度的会话。建议同时监听 reader 与 writer 的结束，传播第一个传输错误，按明确定义回收处理中的任务和队列。可用只关闭输出读端、保留输入端的管道用例验收。

### Q21 · low · A2A 时间过滤器接受不存在的日期

`crates/ma2a-app/src/a2a/time.rs:60`、`:70`，`crates/ma2a-app/src/a2a/list.rs:71`。ListTasks 的 statusTimestampAfter 将非闰年 2 月 29 日或 4 月 31 日作为有效输入；解析只检查日落在 1 到 31，日期算法随后将其归入下个月。调用者得到另一个时间窗口内的结果而非约定的参数错误。实测原解析器将 2023-02-29 转为 3 月 1 日，2024-04-31 转为 5 月 1 日，有效闰日保持原日期。三组日期均为测试输入，源码及实际输出见本轮证据目录的 calendar-probe.rs 与 calendar-results.txt。建议按照月份和闰年验证日期；覆盖非闰年 2 月 29 日、4 月 31 日和有效闰日。

### Q22 · high · Windows 自定义 A2A token 文件没有实施当前用户访问限制

`crates/ma2a-app/src/a2a.rs:38`、`:42`、`:88`，`docs/protocol/a2a-adapter.md:18`。在支持的 Windows 主机上，若操作者把 token 文件放在具有共享继承权限的目录，写入函数只有 Unix 分支设置私有权限，Windows 采用目录继承的 DACL，也没有验证最终文件。可读该目录内容的其他本机账户可能取得代表当前操作者的凭据。回环监听可被同一机器上的其他账户访问。默认受保护状态目录与自定义目录的前提不同。此结论来自条件编译分支及创建调用，未在 Windows 实测。建议从创建时就为文件指定明确的当前用户 DACL，或拒绝无法验证为私有的目的地，并在原生 Windows 的共享父目录下验收。

### Q23 · medium · 同一状态目录中的两个 A2A 监听器会互相覆盖凭据文件

`crates/ma2a-app/src/a2a.rs:39`、`:87`、`:113`。两个实例选择不同可用端口、使用同一状态目录和默认 token 文件时，第二个正常启动会替换第一个的文件。第一个之后正常退出，又会无条件删除第二个仍在使用的文件。先前读过 token 的客户端可继续使用对应实例，新客户端无法按 ready 信息找到正确凭据。证据为监听器生命周期、共享默认文件名及无条件删除。建议按监听实例分配私有文件，或用覆盖整个监听生命周期的排他租约；退出只清理本实例创建的对象。

### Q24 · medium · 托管服务的调用准入能把旧 manifest 的许可用于新 link

`crates/ma2a-runtime/src/hosting/call.rs:114`、`:130`、`:140`，`crates/ma2a-runtime/src/hosting/operator.rs:21`。正在准入的调用先异步取出启用时的注册记录。若在该任务再次被调度前服务被停止、替换并重新启动，随后只按同名服务取当前 ready link，却仍用旧 manifest 检查版本、操作和 max_calls。新实现可收到已不在新 manifest 内的操作，或超过其新并发上限。生命周期锁只覆盖 operator 侧操作；SDK 即使最终拒绝未知操作，也不能补回 Runtime 的配额保证。证据为分开读取的注册快照和 live 状态；未对该交错做时序探针。建议在准入及占槽时验证同一注册代际，或将短暂的取注册与占槽放在同一生命周期临界区。

## 本地集合响应与 React 控制台

### Q25 · medium · 合法的 exposure 集合可以超过 IPC 响应容量而无法列出

`crates/ma2a-runtime/src/ipc/server/execute/services.rs:68`、`:129`、`:247`，`crates/ma2a-runtime/src/api/responses.rs:283`、`:308`，`crates/ma2a-runtime/src/api.rs:81`。管理员依次登记 128 个不同的 root 名称，各指向同一个至少 512 个 ASCII 字节的有效目录。Store 只按 root 名检查绑定（`crates/ma2a-store/src/service_exposures.rs:219`），未触及 256 项上限，但列表光是路径就占满 65,536 字节。附加 JSON 字段后，编码必然失败；这里仅为 snapshot 和 space_list 提供大响应通道。CLI 无法一次列出该合法状态，Web Services 聚合请求也会失败。证据是可接受输入下的字节数下界。建议为 exposure 列表增加有游标的分页或已验证的分片，覆盖多页及单项接近上限的情况。

### Q26 · low · Services 的旧轮询可以覆盖更新的成功结果

`web/src/screens/use-services.ts:18`、`:28`，`web/src/api/services.ts:202`。面板每 3 秒启动一组请求，每组允许等待 5 秒。第一组较慢、第二组先返回时，第一组随后到达的旧数据或超时仍会写入相同 React state，界面倒退或从可用变成失败。卸载标记只区分面板是否仍打开。建议等本轮完成后再调度下一轮，或按轮次拒绝旧结果；用有意错序完成的两个查询验证。

## CI、依赖和运维脚本

### Q27 · high · E2E 脚本会递归清空操作者指定的现有目录

`scripts/run-e2e.sh:5`、`:16`，`.github/workflows/e2e.yml:32`。如果把可配置的证据输出位置设为已有的共享构建目录，脚本在盘点或运行任何测试之前就会递归删除整个目录，其中可含与本次运行无关、无法从版本控制恢复的文件。代码只检查运行次数及 jq 是否可用，没有验证目录归属。审查只核对了控制流。建议在给定父目录下排他创建每轮子目录，或拒绝复用非空目录；用预存普通文件的目录验证文件得以保留。

### Q28 · medium · 当前锁文件使依赖审查门禁失败

`Cargo.lock:448`，`deny.toml:4`，`xtask/src/main.rs:86`，`.github/workflows/ci.yml:50`。使用本次运行取得的公告库检查锁文件时，cargo-deny 将 chrono 0.4.45 判为 unmaintained（RUSTSEC-2026-0543），xtask 在此退出，后续门禁没有通过该入口执行。反向依赖包含 tokio-rustls-acme → iroh-relay 及 Windows 的 wmi → netwatch。此处报告维护状态和实际阻断的检查，未由该公告推导运行时漏洞。建议评估并更新两条上游依赖路径；若需限期接受风险，记录范围、理由和复查日期。验收为使用同一公告集时完整 xtask 可执行至末尾。


## 前三轮修复抽查

| 旧编号 | 目前实现与本轮证据 |
| --- | --- |
| R1、R7 | `crates/ma2a-store/src/agent_output.rs:109` 同时比较 Agent 身份与 generation。`crates/ma2a-store/src/agents.rs:115` 从持久 revision 派生重建后定义的起始代际。Q19 发生于 Process 成功启动、绑定失败后的清理阶段。 |
| R2 | 同目标下载已持有 lease。普通 hard link 路径防覆盖；不支持 hard link 的回退仍留有检查与 rename 的间隙。详见 Q14 后的差量复核。 |
| R4、R6 | `crates/ma2a-agent/src/sdk/task.rs:204` 在结束任务前验证消息的语义边界。前端两个 runner 在当前 HEAD 各通过 149 项。 |
| R11、D1 | 启动会回收未完成重放的容量（`crates/ma2a-runtime/src/store/identity.rs:76`），有实际重启测试（`crates/ma2a-runtime/src/ipc/server_tests/pending_replay.rs:4`）。handshake 分支已跳过全局 replay 锁（`crates/ma2a-runtime/src/ipc/server/dispatch.rs:125`）。 |
| R15、R16、D3 | 替换注册时清理旧版本计数，stop 通过独立取消信号传达（`crates/ma2a-runtime/src/hosting/operator.rs:42`、`:99`）。注销时在同一事务删除该名字的 hosted exposure（`crates/ma2a-store/src/hosted_services.rs:141`、`:152`）。Q24 关注的是准入已读取旧注册快照时的交错。 |

其余旧编号只用作去重依据。没有逐项重跑旧探针。已知 Agent revision 语义以及 D4、D6、D7 和 limitations 中列出的取舍均未计入本轮统计。

## 待确认

- **混合长度的 manifest 分页。** `crates/ma2a-runtime/src/control_sync/pages.rs:98` 在某一代超过本页剩余预算后会继续尝试后一代，而接收端要求连续推进（`crates/ma2a-runtime/src/control_sync/pages/staging.rs:85`）。还缺少一条可由正常成员操作产生、恰好跨过真实协商预算的签名链及其分页结果。应验证发送的第一个不连续页是否可达、下一轮能否自行推进。
- **控制响应写入与准入名额的回收时间。** `crates/ma2a-net/src/control/handler.rs:125`、`:147` 在持有准入凭据时等待写出响应，该次写入没有应用层 deadline。未测出所支持 iroh 传输配置在写端长期背压下的实际回收上界。需以受控传输测试确认，再判断是否要为整个响应写入设总时限。

## 本轮验证

本轮统计 **28 项，critical 0、high 7、medium 16、low 5**。待确认和旧问题复核均在统计外。安全及并发结论以各项所列的生产代码分支、提交顺序和前置条件为依据，未将普通测试通过作为触发这些交错的证据。

在仓库根目录运行以下命令即可复核。本轮在 Linux 上先预构建全部二进制。xtask 通过格式、Clippy 及此前的仓库检查，Rust nextest 在 134 个测试二进制上运行 920 项，全部通过，跳过 0 项。随后 cargo-deny 返回 1（Q28）。后续检查独立执行，cargo-machete、前端 Biome 和 TypeScript 通过；同一套 149 项前端测试在 Bun 与 Vitest 各通过一次。本机 Bun 为 1.4.0，CI 固定为 1.3.5。原生 Windows 与 macOS 行为、发布流程和生产部署没有在本轮执行。

```sh
cargo build --locked --workspace --bins
CARGO_TERM_COLOR=never cargo run --locked -p xtask -- check
cargo clippy --color never --workspace --all-targets --all-features -- -D warnings
cargo deny --color never check
cargo tree --locked -i chrono --target all
cargo machete
(cd web && bun install --frozen-lockfile && bun run check && bun run test)
```

日期探针直接编译原实现，输入仅为三组测试日期。以下命令使用新的证据子目录保存可执行文件。本轮归档仅保留可公开的源码与输出文本。两条无效日期在当前 HEAD 被推进下个月，有效闰日保持原值。

```sh
evidence=.omo/reviews/2026-11-20
probe_dir=$(mktemp -d "$evidence/calendar.XXXXXX")
rustc --edition=2024 --remap-path-prefix="$PWD"=. -C strip=symbols "$evidence/calendar-probe.rs" -o "$probe_dir/calendar-probe"
"$probe_dir/calendar-probe"
```

本轮输出在 `.omo/reviews/2026-11-20/` 下的 `check-summary.txt`、`deny-summary.txt`、`supplemental-results.txt`、`calendar-probe.rs` 和 `calendar-results.txt`。已有的 `rust-results.txt`、`web-results.txt` 来自中断的旧会话，未用于计算本轮结果。

## 覆盖度

深审表示沿表中列出的生产入口检查至边界判定、持久化或清理，并对照协议；抽查表示只读了指定代表路径。没有用测试总数推算源代码覆盖率。

| crate 或模块 | 深度 | 实际覆盖与限制 |
| --- | --- | --- |
| ma2a-core · 身份、Space、邀请、控制对象 | 深审 | 签名链推进、前驱哈希、撤销和重新加入、公开导出、邀请和离场分页、控制游标与对象预算。签名库算法实现未独立审计。 |
| ma2a-core · 其余服务消息与编码 | 抽查 | 选取授权位、帧、路径、时间与大小边界；未逐个走读所有编解码器及 golden vector。 |
| ma2a-store | 深审 | 打开与迁移、完整性校验、在线备份、密钥引用与权限、成员更新、地址高水位及事务、重放、Process、Agent、托管注册与 exposure 行。恢复按仓库提供的能力审查；未实际恢复生产备份。 |
| ma2a-net | 抽查 | 沿 enrollment/control ALPN、read 前准入、地址与中继签名验证、候选查询、relay access 和服务流配额追踪到 Runtime。未独立重现外部中继、多网卡或 NAT 行为。 |
| ma2a-runtime · 成员与控制同步 | 深审 | 离场双方、重试与重新加入、轮次选 peer、分页发送与 staging、回滚和同代分叉判断、整批提交、地址过期。 |
| ma2a-runtime · 本地 IPC 与 Web | 深审 | handshake 和重放、响应大小、登录与会话读取、Origin/CSRF、SSE 撤销和事件期限、HTTP 监听范围及文档承诺。 |
| ma2a-runtime · 服务授权与生命周期 | 深审 | grant/exposure、控制状态变化、Process 恢复、Agent 后台启动与清理、托管注册、准入及替换。其余 actor 调度、统计与诊断路径抽查。 |
| ma2a-services · Files、Transfer、Exec、Shell、Proxy | 深审 | 目录句柄、上传日志与发布、本地辅助文件与 lease、输入输出流、写中断、配额/超时及终态。PTY 与文件系统差异按源码核对。 |
| ma2a-services · Process supervisor | 深审 | 状态文件、失联重连、终态确认、日志分段保留与写入失败。未逐种模拟系统崩溃和磁盘故障。 |
| ma2a-agent | 深审 | stdio 读写生命周期、消息大小与终态，和 Runtime 的实例、存储代际交叉检查。示例 Agent 业务逻辑只抽查。 |
| ma2a-service | 抽查 | SDK 注册覆盖、ready、stop、重连、caller 帧与 Runtime 注册状态的配合。未逐一模拟第三方 handler 的错误路径。 |
| ma2a-app · A2A | 深审 | bearer 文件创建与删除、监听器生命周期、HTTP/RPC 入口、任务列表过滤、SSE 与 Agent 边界。日期用原函数探针验证；未执行外部 A2A 互操作测试。 |
| ma2a-app · CLI、E2E | 抽查 | 授权标志、服务清单、离场和托管调用入口、E2E 前置构建及证据脚本。已运行当前 workspace 全套测试，未逐个评审每个 E2E 断言。 |
| web/src | 抽查 | 服务数据的 HTTP 聚合、刷新竞态、访问会话和 SSE 接口契约、旧前端 runner 修复。布局、图形、无障碍交互未系统走查。 |
| xtask | 抽查 | 检查编排、失败中止位置、格式、依赖、前端和发布检查入口；已执行 check，未执行完整 dist。 |
| .github 工作流、scripts | 抽查 | 原生平台及质量 job、E2E 证据目录及校验、发布元数据生成入口。未执行云端工作流、签发证明或实际发布。 |
| README、docs 与 protocol | 深审 | 将架构、安全、限制、持久化、邀请与控制、本地 API、远程服务、Process、Agent、A2A、托管服务的相关承诺与上述入口对照；平台安装与操作示例只抽查。 |
| 原生 Windows / macOS 行为、外网部署、前端视觉与无障碍 | 未审 | 没有在原生环境、外网部署或真实浏览器辅助技术下验证；Windows 条件代码发现均单列依据。 |

