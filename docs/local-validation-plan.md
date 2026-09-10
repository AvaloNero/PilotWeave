# PilotWeave 本机自动验证

本机入口已实现。A 是代码/浏览器回归，B 是隔离的真实 Windows WebView/IPC 集成测试，C1 使用现有环境观察真实能力，C2 为专用空白用户保留可接续验收入口。维护者本轮选择现有环境，未创建用户或 VM，未执行 C2 安装验收。各次实际结果以仓库外的报告为准；本文不把未执行的项目算作通过。

完成这些验证不等于完整 MVP 达标；仍按 [实现规范 §21](mvp-implementation-spec.md#21-final-acceptance) 判定。P0/P1 之外的 Resources 等缺口不由本套件补齐。

## 运行入口

要求 Windows x64、交互桌面、PowerShell 7、Node.js 24 或更新、Rust/MSVC 工具链，以及 Edge/WebView2。运行时重新探测 WebView2 版本；WebdriverIO 固定为 9.31.7，匹配版本的 Edge WebDriver 放在任务专用工具目录。缺少 npm 构建/驱动依赖时，脚本从锁文件安装，使用 `--ignore-scripts`。

```powershell
pwsh -NoProfile -File .\scripts\verify-local.ps1 -Mode Regression
pwsh -NoProfile -File .\scripts\verify-local.ps1 -Mode NativeIsolated
pwsh -NoProfile -File .\scripts\verify-local.ps1 -Mode PrepareLive -LiveTarget HostObserve
pwsh -NoProfile -File .\scripts\verify-local.ps1 -Mode LiveAcceptance -LiveTarget HostObserve -RunId <上一步ID> -Resume
```

A 的既有浏览器套件使用现有 Playwright。自动查找 Codex 的捆绑运行库；其他环境须把 `PILOTWEAVE_PLAYWRIGHT` 设为已安装 Playwright 模块的绝对路径。不会把浏览器 mock 结果当作原生结果。

`PrepareLive` 构建默认 feature 的 current-user NSIS 包，复制 EXE/安装包，记录源码指纹和 SHA-256，准备范围清单，**不安装**。`scripts/package-local.ps1` 是这一打包入口的快捷方式。

报告目录为 `%LOCALAPPDATA%\PilotWeave-validation\runs\<RunId>`。`-ToolRoot` 可指定非链接的独立工具目录。退出码：0 通过、1 失败、2 阻塞。每步结束写入 HTML、JSON 和 JUnit；未结束显示 RUNNING。重跑同一 ID 会保留以前的报告到 `attempts/`，不覆盖失败历史。执行中修改源码会使“Source tree remained unchanged”失败，防止把混合源码验证当作同一版本。

## 分层与边界

| 层 | 实际实现 | 证据含义 |
| --- | --- | --- |
| A / Regression | 规定的 Web、fmt、默认/隔离 feature Rust 测试与 Clippy、工具测试、浏览器 mock、默认 Release、测试参数负向检查、diff 与输出边界检查 | 代码与浏览器契约；默认构建不带隔离入口 |
| B / NativeIsolated | 新建专属根，启动隔离 EXE，真实 DOM 和 Tauri IPC 驱动 Rust/SQLite/事务/解析/估算，重启与故障恢复 | 真实原生集成；外部客户端、网络、凭据仍为替身 |
| C1 / HostObserve | 默认 EXE、真实组件/账号观察、公开价格、官方只读 quota/models、独立授权状态、重启 | 仅证明报告中观察到的真实能力，不证明安装或日志导入 |
| C2 / DedicatedUser | 专用空白用户预检、默认包安装、先单组件后剩余组件、官方登录入口、人工步骤接续、独立 Billing、用户启用的新日志导入与重启 | 代码入口已具备；本轮未执行，干净 Windows 验收仍待完成 |

B 使用 WebdriverIO 直接连接 Microsoft Edge WebDriver，启动 PilotWeave 的 WebView2，未启用内嵌 WebDriver 插件，也不要求安装 tauri-driver。驱动与 WebView2 版本必须匹配，失败记录 BLOCKED_DRIVER，不降级成 mock。[Microsoft 官方 WebView2 WebDriver 说明](https://learn.microsoft.com/en-us/microsoft-edge/webview2/how-to/webdriver)

本机驱动启动时会把附着的 WebView 导航到 `about:blank`。驱动封装记录初始 URL，再导航到应用编译的 `http://tauri.localhost` 资源协议，等待真实导航栏和 IPC；页面、handler、数据库均来自 EXE。此证据说明“经 WebDriver 启动和加载应用”，不替代独立的安装后双击/开始菜单启动验收。

## B 的隔离实现

Cargo feature `local-e2e` 默认关闭。测试 EXE 必须提交与 `context.json` 的 UUID 一致的专属绝对根，拒绝缺失上下文、父路径、链接和 junction；默认 EXE 在打开存储前拒绝所有 `--local-e2e*` 参数。没有测试 IPC 命令或生产环境变量开关。

- State、Usage SQLite、Known Folder/home、WebView 数据统一限制到本次根。文件安全入口继续校验重解析点。
- SecretBackend、包括 `secrets::observe` 在内的访问全部重定向到私有假凭据。HKCU 投影使用根内的字节夹具，真实广播关闭。
- 客户端发现、安装执行及重新检测共用固定标记；ProcessRunner 只允许固定包 ID/扩展/只读身份探测，不执行真实子进程。登录替身不打开真实客户端。
- GitHub 授权/Billing、公开身份查询、价格都映射到本次回环服务；RPC 仅允许四种只读方法。测试 HTTP 显式禁用环境代理，未知目标失败关闭；正式 HTTP 保留 HTTPS 限制与现有代理行为。
- 固定、一次性的故障点覆盖 journal、写入、audit、导入提交、价格/runtime 保存、安装/登录。业务状态转换、指纹检查和恢复实现不被替换。
- 源文件只来自脱敏夹具，数据库保存元数据；敏感哨兵检查覆盖普通 state、Usage DB 以及返回 DTO。假密钥投影和回滚数据保留在私有目录，不作为报告导出。

B 覆盖表（具体通过情况见每次报告）：

| 编号 | 原生场景 |
| --- | --- |
| B00–B04 | 非法隔离根拒绝、Home、单组件/剩余安装与重检测、Connection UI/native 校验、保留注释/外国组/原始备份的真实临时部署 |
| B05–B08 | 审核/一次性计划、无操作字节与时间戳、目标/凭据/状态变化、重启、部分失败补偿、审计失败、进程中断、外部修改保护、新 Keep current 审核 |
| B09–B11 | 真实解析器、重复导入、不完整尾行、轮换/截断、缺失值/零、精确 0.00075、价格 A→B→A 与历史绑定 |
| B12–B14 | 官方 quota 零/未知/空/协议/授权状态、Billing 错误分类和月份隔离、保存失败收尾、并发拒绝与取消 |
| B15–B18 | 隐私哨兵、opt-in 重启、清理确认/源文件保留、跨 2^53 聚合字符串、导入提交后中断与恢复去重 |
| B19–B22 | 损坏/未来 SQLite 不重建、主状态与 last-good 恢复、跨进程写锁、退出成功但组件未发现仍为失败 |
| B23–B25 | 用户确认与 Verified 区分、确认过期/证据变化、授权更改丢弃迟到 Billing、模型别名 v2 的导入/过滤/绑定 |
| B26 | VS Code CLI 替身强制检查入口脚本和 Node 模式；探测失败为 Unknown，单项/全量安装计划均拒绝 |
| B27 | 实际 Tauri IPC 请求本地模型目录夹具；URL/key 发现、筛选追加、保存/凭据复用、HTTP 错误及关闭后的迟到响应 |
| B28 | 首页原位新增/编辑连接、保存后选中、模型摘要刷新；预览部署前后均不产生配置部署 |
| B29 | 首页只预览选定客户端；VS Code 关闭/已打开/重复登录操作均不调用进程启动，保持原生计划一次性消费 |

计划 TTL、更多目标消失/不可读的恢复组合、客户端输入语义/共享 runtime 去重、价格旧响应等还有既有 Rust/浏览器回归；并非每项都另有原生 UI 重复用例。结果应引用相应层级。

## C1：现有环境

本轮按维护者选择执行 HostObserve。它会读组件存在性和官方身份观察，刷新公开价格及 Copilot 只读 RPC；PilotWeave 自身会写状态/缓存，不能称为零磁盘写入。报告分别记录检查结论和实际能力状态；版本未暴露时仍是 null，不能推断版本。

此模式不安装组件、不启动登录、不写客户端配置、不启用日志源、不读历史会话内容、不借用客户端 token。会比对 Connection、部署历史和源 opt-in 前后状态。个人 Billing 的独立授权缺失时记录 BLOCKED，不制造零用量，也不自动跳到其他凭据。

2026-09-10 补充：旧 C1 仅记录组件状态，漏掉了 GUI 探测打开 VS Code 窗口以及内置 Copilot 被误报为缺失的问题，不能作为这两项已验证的证据。现在 C1 先直接启动无参数默认 EXE（不经 WebDriver），再执行原有 IPC 验收，并用只读窗口观察器覆盖启动、三次重复检测和重启。观察器每 100 ms 枚举可见顶层窗口，只返回程序名/PID/窗口句柄；既有 VS Code 窗口作为基线保留，出现新增 VS Code/Insiders 窗口即失败。未知/缺失组件记录 BLOCKED。可在私有验收清单的 `expectedComponents` 中记录独立核对的组件 ID/status/version，并与实际 IPC 结果逐项比对。检测实现和已核对版本见 [VS Code 检测](vscode-detection.md)。

当前 CLI 导入只能选择 source，日期查询不能限制导入时读取旧日志；因此真实新日志导入仍放在 C2 的空白用户内执行。

2026-09-11 登录路径补充：启动/重扫时没有新增窗口，不能证明点击登录同样安全。实际旧版登录调用一次 `Code.exe --reuse-window` 后，VS Code 恢复了大量保存窗口。官方 [windowsMainService](https://github.com/microsoft/vscode/blob/main/src/vs/platform/windows/electron-main/windowsMainService.ts) 的初始启动逻辑还会恢复 hot-exit 备份；[app.ts](https://github.com/microsoft/vscode/blob/main/src/vs/code/electron-main/app.ts) 的 `--new-window` 不能覆盖这些恢复分支。已与本机 1.137.0 / 645f29cc3176500b4b5762ba887cf2a7f0ffdf2c 安装代码核对。修复后登录不再启动 VS Code，只定位已有窗口；未打开时显示手动指引。B29 的已打开窗口为替身；真实环境另测关闭状态的重复登录并观察新增窗口/进程，不据此宣称真实登录或已打开窗口的聚焦通过。既有恢复设置和用户文件保持原状。

窗口适配器先检查窗口类，不查询标题或标题长度；[GetWindowTextLengthW](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getwindowtextlengthw) 对本进程窗口可能发送同步消息。恢复最小化窗口使用 [ShowWindowAsync](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-showwindowasync)，避免等待无响应的客户端。Windows 单元回归创建本测试进程拥有的离屏窗口，故意不处理消息，验证定位在 2 秒内返回并由窗口所属线程清理；不创建或操作真实客户端窗口。

## C2：后续专用用户验收

不自动创建用户、启用虚拟化或操作 VM。维护者准备空白专用 Windows 用户后，在其交互桌面执行：

```powershell
pwsh -NoProfile -File .\scripts\verify-local.ps1 -Mode PrepareLive -LiveTarget DedicatedUser
pwsh -NoProfile -File .\scripts\verify-local.ps1 -Mode LiveAcceptance -LiveTarget DedicatedUser -RunId <ID> -Resume
```

首次预检拒绝已有 Copilot、VS Code 或 PilotWeave 用户数据；通过后只建立绑定当前 SID 与 Run ID 的标记。它不会通过卸载/删配置来制造空白用户。每个需人工动作会写 `live-manifest.json` 的 `pendingAction` 和审核范围。完成该动作后用对应的 `-CompletedAction` 接续；仅 `-Resume` 或等待不会批准操作。范围改变使原批准失效。

SID 获取或格式校验失败会记录 BLOCKED，且不会登记用户或启动安装。登记标记保留在该专用用户下：同一 Run ID 可继续，其他用户或 Run ID 会得到 BLOCKED 及接续提示。请用原 Run ID 在原用户下 `-Resume`；一轮新的干净环境验收使用另一个空白用户。不要删除标记把已有客户端数据的用户重新当成空白环境；标记损坏或不可访问同样阻塞并保留现场。

可接续动作：InstallPackage、InstallFirstComponent、InstallRemainingComponents、SignIn、AccountConfirmation、ProviderSetup、GitHubAuthorization、UsageSources。自动安装使用审核范围内的原生计划；安装后核对实际文件 SHA-256、组件重检测。UAC/MFA、官方客户端登录、手动 provider 配置及独立授权由用户在官方 UI 完成，驱动不输入密码/token，不记录认证页面或按键。

ProviderSetup 会检查真实部署状态及 Copilot app 的手动确认；它仍不证明客户端已经成功发送请求或新终端继承了环境。最小真实请求、回环 provider、各客户端版本和新环境读取的独立核验仍须在专用环境补验，报告不能把这些人工前置动作算作脚本已验证。UsageSources 只在专用用户下启用的源进行导入两次及重启比对，不向模型自动提交提示词。VS Code 须使用 metadata export、captureContent=false。

本轮 C2 为 deferred/BLOCKED；没有洁净安装或真实新日志导入的通过结论。

## 报告与清理

每次记录 Git HEAD、工作树 SHA-256、dirty 状态、配置、EXE/安装包/驱动 SHA-256、实际 WebView2 版本。报告和夹具位于只授权当前用户及 SYSTEM 的任务目录。可分享 `report.html`、`summary.json`、`junit.xml` 和已检查的 B 层截图；不要分享整个 run/private 目录、原始数据库、源文件、journal 或凭据。

PowerShell 在释放启动握手前把 Node 放入 Windows Job Object；父进程退出、异常或被终止后，系统关闭本次后代。命令超时只终止拥有的 PID 树。工具回归实际检查关闭 Job 和终止父进程两种路径的子进程/回环端口释放，B 再检查正常收尾。最大运行时间 45 分钟；单命令默认 20 分钟。每次结束保留私有失败现场，不自动递归删除用户资料。

本轮验证修复了远程 Usage 作业在异常保存后遗留 InProgress，以及 OpenRouter 新增前导 `~` latest 别名导致目录整体拒绝的问题。别名仍保持完整身份、版本化解析和精确 decimal；不猜测实际模型，既有价格快照保持不变。来源见 [Usage 数据源文档](usage-sources.md)。
