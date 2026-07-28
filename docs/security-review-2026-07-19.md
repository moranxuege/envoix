# Envoix 安全审查工作稿（2026-07-19）

> 状态：本地工作稿，供团队讨论，不代表最终设计或已经批准的 GitHub Issue。
>
> 本次仅记录核查结果和建议，没有修改 Issue、标签或回复。

## 1. 当前团队初步方向

已经形成但仍需确认参数的方向：

1. 房间短码保留可用于匹配的数字前缀，后缀不再使用原来的两个单词，改为密码学安全随机生成的 Base36 或 Base62 字符串。
2. 服务端增加限流：HTTP 接口使用 middleware；真正承载房间 JOIN 的 iroh/QUIC broker 必须另设接入层限流。

仍待讨论：

- 采用 Base36 还是 Base62，以及最终长度。
- 网络中断是否自动恢复、`90 秒 / 5 attempts` 是否合适。
- room continuation 是否安全、是否继续使用同一个 room code。
- 同一用户旗下设备的持久化传输身份、长期重试和撤销机制。

## 2. 威胁模型与安全边界

当前至少需要考虑以下攻击者：

- 知道或猜到数字房间前缀、能够反复尝试短码的远程攻击者。
- 与用户处于同一局域网或 BLE 范围内，能够监听、伪造或抢先连接的附近攻击者。
- 能访问明文 HTTP、代理、日志平台或备份数据的网络及运维侧攻击者。
- 获得共享目录写权限，能够预置符号链接、临时文件或恶意文件名的本地攻击者。
- 已经通过配对但发送超大、畸形或耗时数据，试图耗尽磁盘、内存或连接资源的对端。
- 供应链、CI、签名密钥或发布制品被替换的攻击者。

这里应明确：短码当前证明的是“对方知道同一份秘密”，不是“对方是某个已知用户或已信任设备”。持久化设备传输必须建立独立的长期设备身份，不能把房间短码当设备身份使用。

## 3. 已确认的安全基础

- 当前短码和邀请 token 使用系统密码学随机源，而不是用户自己选择；随机源本身没有发现明显问题。
- rendezvous 使用 SPAKE2，并通过角色、版本、随机数、密钥确认和 QUIC exporter 绑定降低中间人及跨连接转发风险。
- endpoint descriptor 使用 AEAD 加密，数据面再次认证；没有 exporter 的自定义 transport 默认不能悄悄绕过认证。
- 文件清单和数据帧已有数量、长度、路径、偏移、chunk 大小及整数溢出校验。
- 接收端校验文件 BLAKE3；恢复传输时会重新散列已有前缀，而不是盲信 sidecar。
- 临时文件最终落盘尽量使用原子或不覆盖语义；Android 组件的导出范围和备份设置已有基础限制。
- 日志读取接口默认要求 operator token，没有配置时倾向于 fail closed。

这些基础值得保留，但不覆盖下列发现。

## 4. 短码 v2：初步方案和兼容要求

### 4.1 当前短码的问题

当前格式为 `6 位数字 nameplate + 两个 64 词词表中的单词`：

- 数字前缀约 `19.93 bit`，会发送给 broker 用于匹配，不应视为秘密。
- 两个单词一共只有 `64 × 64 = 4096` 种，即 `12 bit` 的隐藏搜索空间。
- 完整组合约 `31.93 bit`。随机碰到完全相同完整短码的概率不高，但攻击者若已知数字前缀，只需在线枚举 4096 个后缀。
- SPAKE2 能阻止离线验证密码，不会自动阻止持续的在线猜测。
- broker 当前匹配的是数字前缀和相反的 JOIN intent。数字前缀碰撞会造成误配和可用性问题；若完整短码也相同，双方可能认证到错误的陌生人。
- 文档称房间码是 single-use，但服务端目前只让单个 waiter 超时，没有全局消费并作废某个短码；同一短码可以重新加入。

因此，“单词来自程序词包且由系统随机选择”比人工口令好，但词表大小仍然决定了后缀只有 12 bit，不能消除在线猜测和抢房风险。

### 4.2 候选编码

下表中的“后缀熵”是在攻击者已经知道数字前缀时真正剩余的秘密强度；“完整熵”只适用于连前缀都不知道的情况。

| 编码 | 后缀长度 | 后缀熵 | 加 6 位前缀的完整熵 | 主要取舍 |
| --- | ---: | ---: | ---: | --- |
| 小写 Base36 | 8 | 41.36 bit | 61.29 bit | 较短，但面向长期或高频在线攻击偏低 |
| 小写 Base36 | 10 | 51.70 bit | 71.63 bit | 推荐候选；适合手输，大小写不承载额外信息 |
| Base62 | 8 | 47.63 bit | 67.57 bit | 更短，但大小写容易输入或识别错误 |
| Base62 | 9 | 53.58 bit | 73.51 bit | 熵与 Base36×10 接近，适合主要依赖复制/扫码的场景 |
| Base62 | 10 | 59.54 bit | 79.47 bit | 更强，但手输体验最差 |

当前建议候选为：

```text
123456-k7m4q9v2dx
```

即 `6 位数字 + 10 位小写 Base36`。最终选择仍须用真实设备输入测试、碰撞/负载测试和限流策略共同确认。

### 4.3 实现要求

- 生成器只生成 v2；在明确的迁移窗口内，解析器同时接受旧 `digits-word-word` 和新 `six-digits-base36-suffix`。
- 新格式严格校验分段数、数字前缀长度、后缀长度和字符集；不得接受任意 Unicode、额外空白或模糊变体。若 Base36 的输入层允许大写，应只做明确的 ASCII 小写规范化，wire canonical form 始终为小写。
- 如果选择 Base62，不能静默改大小写；如果以手输为主，优先小写 Base36。
- 使用系统 CSPRNG 和无偏采样；不要直接对随机整数 `% 62`，应使用 rejection sampling 或经过验证的 uniform API。
- 字母表、长度和协议版本必须定义为常量，并为随机源失败、格式拒绝、往返解析和兼容迁移增加测试。
- 日志、指标和错误信息只能记录数字前缀的必要片段，不得记录秘密后缀或完整邀请。
- v2 增强的是秘密后缀。数字 nameplate 的冲突和抢占仍需 broker 的房间分配、等待者上限、TTL 和限流解决。
- room code 应建立明确生命周期：创建、等待、匹配、成功消费、失败冷却、到期作废。不能只依赖客户端 UI 声称 single-use。
- 安全续传应在首次 PAKE 成功后派生并保存独立、高熵、绑定 `transfer_id + peer/device identity` 的 resume credential；不要长期重复使用已经暴露或过期的房间短码。

### 4.4 必须有的验收测试

- legacy 与 v2 格式均可在迁移期解析，生成器只输出 v2。
- v2 严格拒绝错误长度、字符集、分隔符、大小写策略和尾随字符。
- 固定样本的编解码和跨 Rust/Android/Apple 互操作一致。
- 随机源失败时明确失败，不生成降级码；统计测试没有明显位置偏差。
- 并发生成、房间冲突和恶意枚举的负载测试有可重复结果。
- 所有结构化日志和上传报告通过 secret-canary 测试，确认完整短码不会泄露。

## 5. 限流不是一个 middleware：需要两层防线

服务端存在两条独立入口：

1. Axum HTTP(S)：日志和 receipt mailbox。
2. iroh/QUIC：真正的房间 JOIN、等待和匹配。

因此只给 Axum 增加按 IP middleware，无法限制短码猜测、房间抢占和 JOIN 洪泛。

### 5.1 HTTP middleware

要求：

- 从 socket peer address 获取来源 IP；只有请求确实来自配置的可信反向代理时才接受 `Forwarded` / `X-Forwarded-For`。
- `POST /logs`、receipt `POST` 和 receipt `GET` 分桶，不共享一个粗粒度全局计数器。
- 使用带 burst 的 token bucket，超限返回 `429 Too Many Requests` 和 `Retry-After`。
- 在进入业务处理和分配大块内存之前限制请求体；当前日志单次 `64 MiB` 明显高于客户端约 `480 KiB` 的实际需求，建议先降至 `1 MiB` 再根据数据调整。
- receipt key 应严格校验为协议规定的固定长度小写十六进制，而不是只限制最大 128 字符。
- 指标记录 endpoint、结果、桶类型和匿名化来源，不记录 bearer token、receipt key、完整 room code 或邀请。
- 限流表必须有 TTL 和最大条目数，避免攻击者用海量 IP/代理地址反向耗尽限流器内存。

仅作为压测起点、尚未批准的建议值：

| 入口 | 单 IP 持续速率 | burst | 备注 |
| --- | ---: | ---: | --- |
| `POST /logs` | 3 次/分钟 | 5 | 日志上传不是高频正常路径 |
| receipt `POST` | 20 次/分钟 | 40 | 需覆盖正常重试与多传输 |
| receipt `GET` | 60 次/分钟 | 120 | 轮询客户端必须带退避和抖动 |

这些数字必须经真实四设备并发、NAT 下多用户共享 IP、弱网重试和恶意流量测试后确定，并通过配置常量管理，不能写成散落的魔法数字。

### 5.2 iroh/QUIC broker 接入层

当前已有全局连接上限、握手/JOIN 超时、waiter TTL 和全局 waiting-room 上限，但没有 per-IP、per-endpoint、per-room 限流，也没有每房等待者上限。

最低要求：

- 每个 room/nameplate 最多保留 `1 个 sender waiter + 1 个 receiver waiter`；同 intent 的额外等待者应被明确拒绝，而不是无界追加。
- 同时按 room/nameplate、iroh endpoint ID、全局容量限流；能够可信获得直接来源 IP 时再增加 per-IP 桶。
- relay 场景可能只看到中继地址，不能把所有 relay 用户错误地当成同一个 IP 封禁。
- 为单一 room 在有效期内的失败匹配/认证尝试设置上限和冷却。例如 `5 分钟最多 6 次`可以作为测试起点，不是最终策略。
- 保留全局并发上限，并为等待队列、每 endpoint 占用、拒绝原因和匹配失败建立指标与报警。
- 协议应返回结构化 `RateLimited { retry_after }` / `RoomBusy` / `Expired` 等原因；版本化新增响应，并为旧客户端设计可预期的降级行为。
- 对正常网络恢复使用指数退避、随机抖动和单飞请求，避免双端同时密集重试形成惊群。

建议的压测起始值（待批准）：单 endpoint `10 JOIN/分钟，burst 20`；可信单 IP `30 JOIN/分钟，burst 60`；单 room `6 次匹配尝试/5 分钟`。任何最终值都必须证明不会误伤校园网、家庭 NAT、共享 relay 和四设备并发。

### 5.3 限流验收测试

- 直连、NAT、代理和 relay 四类来源的身份提取行为清楚且可测试。
- 超限一定返回可机器解析的原因与 `retry_after`，客户端遵守退避；旧客户端不会无限紧循环。
- 同房间、同 endpoint、同 IP 和全局洪泛分别触发正确的桶。
- 单个攻击者不能让 waiter 列表、限流表、日志存储或 receipt 存储无界增长。
- 多个合法用户共享一个出口 IP 时仍能完成正常发送和恢复。
- 被限流、房间忙、认证失败和网络中断不会被前端统一显示成同一种“连接失败”。

## 6. 主要安全发现与改进项

### P0：启用或公开发布前应解决

#### SEC-001：BLE 邀请交接当前没有认证和保密

Android 和 Apple 的 BLE rendezvous 协议明确使用 `auth=none`，完整 `envoix://pair/...` 邀请通过可写 GATT characteristic 发送，应用层没有加密或对端认证。恶意附近设备可冒充服务诱使发送者交出房间秘密，或向接收者注入伪造邀请；若 BLE 链路层未受保护，秘密还可能被监听。后续 SPAKE2 无法挽回已经从 BLE 泄露的密码。

短期：BLE 仅用于发现，将邀请交接功能关闭或置于默认关闭的实验 feature flag。

长期：先进行经过认证的临时密钥协商，在双方屏幕比较并确认 SAS，再用 AEAD 加密邀请；距离、RSSI、设备名和“碰一碰”本身都不能当身份认证。

另外应给 BLE 分片 assembler 增加容量、TTL、重复/重放抑制和 per-central 限流。

#### SEC-002：短码秘密空间、生命周期及 broker 在线猜测防护不足

按第 4、5 节实现 v2、兼容迁移、真实 single-use/expiry 状态机和 broker 多维限流。新的长后缀不能替代限流，限流也不能替代足够熵。

#### SEC-003：HTTP 日志与 receipt 入口可被滥用

当前入口没有 per-IP 限流；日志请求体上限过大；receipt key 校验宽松，存储槽位可能被占满。按第 5 节增加 middleware、请求体前置限制、固定格式校验、存储配额和同值幂等语义。

#### SEC-004：生产路径允许明文 HTTP 或 HTTPS 失败后回退 HTTP

Apple 诊断上传存在 HTTPS 失败后尝试 HTTP 的逻辑，部分客户端也接受任意 HTTP 地址。生产环境必须对非 loopback 地址强制 HTTPS，证书/TLS 失败应 fail closed；本地开发例外必须显式配置。即使 receipt blob 已加密，key、时序、访问模式和可用性仍会泄露或被篡改。

#### SEC-005：锁定依赖中存在已公开 RustSec 项，CI 没有漏洞门禁

2026-07-19 使用 OSV/RustSec 数据核查 `Cargo.lock`，发现：

- `crossbeam-epoch 0.9.18`：RUSTSEC-2026-0204，修复版本 `>=0.9.20`。
- `quick-xml 0.39.4`：RUSTSEC-2026-0194、RUSTSEC-2026-0195，修复版本 `>=0.41.0`。
- `paste 1.0.15`：RUSTSEC-2024-0436，停止维护。
- `rustls-pemfile 2.2.0`：RUSTSEC-2025-0134，停止维护。

这些命中不等于全部可被 Envoix 远程触发，但发布前必须升级、替换或完成可追溯的不可达性评估。CI 增加 `cargo audit` / `cargo deny`、定期运行和依赖更新流程。

当前锁定 `iroh 1.0.0`，可评估升级至 `1.0.2` 并跑完整互操作回归；1.0.2 包含 transport lane 公平性和 relay 非法消息处理改进。

参考：[RUSTSEC-2026-0204](https://rustsec.org/advisories/RUSTSEC-2026-0204.html)、[RUSTSEC-2026-0194](https://rustsec.org/advisories/RUSTSEC-2026-0194.html)、[RUSTSEC-2026-0195](https://rustsec.org/advisories/RUSTSEC-2026-0195.html)、[RUSTSEC-2024-0436](https://rustsec.org/advisories/RUSTSEC-2024-0436.html)、[RUSTSEC-2025-0134](https://rustsec.org/advisories/RUSTSEC-2025-0134.html)、[iroh 1.0.2 release](https://github.com/n0-computer/iroh/releases/tag/v1.0.2)。

### P1：论文冻结前尽量完成或明确限制

#### SEC-006：接收端缺少用户级资源配额

协议帧有上限，但缺少接收方自己的单文件大小、会话总字节、文件数、空余磁盘安全线和并发写入策略。已认证恶意对端仍可耗尽磁盘或 CPU。应在创建临时文件前检查并向用户展示总量，传输中持续执行硬上限。

#### SEC-007：持久化记录和 token 的静态保护不足

传输记录可能包含 room code/token、invite、peer descriptor、路径和平台附加信息；iOS 还将手工 token 放在 `UserDefaults`。应减少持久化秘密，使用 Keychain/Keystore 包装密钥加密必要记录，设置文件保护/Unix 权限、备份排除、终态清理和保留期限。

#### SEC-008：临时文件和原子落盘仍有符号链接/TOCTOU 风险

确定性 `.part`、state、receipt 和 record 临时路径的部分打开方式会跟随预置符号链接；manifest 存在检查后再打开的竞态；不支持 hard link 的 fallback 可能失去 no-replace 保证。共享/CLI 目录应使用目录 capability、`openat`/`O_NOFOLLOW`、随机临时句柄和平台原子 no-replace rename；无法保证时应安全失败。

#### SEC-009：日志与诊断报告仍可能泄露隐私和关联信息

日志可能包含 IP/地理信息、room 前缀、transfer ID、文件名、路径和自由文本。receipt key 目前还会打印前缀。应建立结构化字段 allowlist、中央脱敏器、secret-canary 测试、上传前用户确认/预览以及明确的服务端保留和访问政策；GeoIP 应仅开发启用或显式 opt-in。

#### SEC-010：传输阶段缺少统一应用层 deadline

认证有 deadline，但多个等待对端 frame 的传输阶段主要依赖底层 transport 行为；自定义 transport 不一定提供相同超时。应为各协议阶段定义常量化 deadline，并要求 transport adapter 提供 channel binding、取消和 deadline 语义，通过统一 conformance suite 验证。

#### SEC-011：跨平台文件名与显示欺骗防护不完整

清单已拒绝 traversal 和控制字符，但仍需处理 Unicode 双向控制、平台保留名、尾随点/空格、冒号及过长单组件。wire 原始名称与本地安全映射名应分离，UI 必须使用不会造成路径/扩展名欺骗的显示方式。

#### SEC-012：receipt mailbox 的写入能力和抗占位能力有限

任何知道 key 的人都能覆盖 blob，攻击者也可填满有限槽位。AEAD 阻止伪造有效成功回执，但不能阻止删除、垃圾覆盖、流量分析或 DoS。近期先做严格 key 校验、TLS、限流、相同 blob 幂等和容量隔离；长期考虑分离读/写 capability 或签名写入。

#### SEC-013：持久化同用户设备需要独立信任模型

为设备创建长期密钥对和稳定 device ID，首次信任通过安全短码/二维码/SAS 建立；后续请求签名并绑定账号/设备、transfer ID 和过期时间。还必须支持密钥轮换、设备撤销、丢机处理和可信设备 UI。长期 attempts 政策应属于此通道，不能复用临时 room 的重试上限。

#### SEC-014：发布制品和 CI 供应链需要加固

当前 Android release workflow 仍产出 debug-signed APK；桌面制品缺少签名、校验和、SBOM/来源证明；部分 Actions 使用可变 tag。应使用受管 release key、SHA 固定 Actions、最小 job 权限、`--locked`、受保护 tag、制品 checksum/签名、SBOM 和 provenance。

#### SEC-015：测试与文档没有完全跟上实现

- nearby discovery 文档仍称二维码/手输邀请是必需路径，但 BLE handoff 已传输完整邀请。
- invite 文档称短码 single-use，服务端尚未全局执行。
- 部分 token 熵说明与当前 128-bit 随机 token 不一致。

需要把这些列为“文档/实现漂移”，在论文安全声明中只描述已经实现并测试的保证。增加 malformed frame、fuzz/property、错误角色、重放、限流、磁盘不足、崩溃恢复、relay/NAT 和跨设备对抗测试。

## 7. 建议的 15 天安全收敛顺序（未批准）

适合四人并行，但合并点和协议常量必须由一人统一把关：

| 时间 | 主线 A：协议/服务端 | 主线 B：BLE/发现 | 主线 C：平台/存储 | 主线 D：测试/发布 |
| --- | --- | --- | --- | --- |
| Day 1–3 | 定稿短码 v2 格式与兼容；broker 限流设计 | 默认关闭不安全邀请 handoff，保留发现 | TLS fail-closed；移除 UserDefaults token | 建立安全回归清单和依赖审计 CI |
| Day 4–6 | 实现 broker + HTTP 限流、房间 waiter 上限 | BLE 边界/分片 DoS 测试；安全 handoff 只做设计或最小原型 | 日志 body/key 校验、记录最小化 | 限流/NAT/relay/并发压测 |
| Day 7–9 | single-use/expiry 状态和结构化错误 | Android/Apple 发现互操作 | 接收配额、空盘检查、最低静态保护 | 依赖升级、协议互操作和恶意输入测试 |
| Day 10–12 | 修复压力测试暴露的问题 | 只修 bug，不扩新发现协议 | 符号链接/落盘高风险项修复或明确限制 | 跨真机回归、发布签名/制品检查 |
| Day 13–15 | 冻结协议和参数 | 冻结 | 冻结 | 论文证据、威胁模型、已知限制；只修阻断 bug |

不建议在最后阶段同时交付“安全 BLE 邀请交接”和“持久化可信设备”两个完整新信任模型。BLE 发现可以保留；未经完整认证验证的 BLE handoff 应明确关闭。持久化设备能力可以作为后续 roadmap，但当前先把身份、密钥、撤销和重试边界写清楚。

## 8. 待团队逐项确认

- [ ] v2 选择小写 Base36×10，还是 Base62×9/10？
- [ ] 是否保留 6 位数字 nameplate；由客户端随机还是服务端预留唯一前缀？
- [ ] legacy 短码兼容到哪个版本/日期，如何提示旧客户端升级？
- [ ] room 在何时被视为消费：匹配、PAKE 成功，还是首个 transfer 建立？失败时是否允许重新占用？
- [ ] broker 和 HTTP 的限流起始值、可信代理来源及 relay 公平策略。
- [ ] `90 秒 / 5 attempts` 的适用范围；不得与持久化可信设备策略混用。
- [ ] room continuation 是否取消，或改为首次认证后签发独立 resume credential。
- [ ] BLE 当前是否确定为 discovery-only；安全 handoff 是否移入下一阶段。
- [ ] P0/P1 项的负责人、完成证据和论文中需要披露的已知限制。
