# 小米互联服务发现与连接路径分析（2026-07）

## 结论

当前证据不支持“关闭蓝牙后仍能传输，因此使用了 Wi‑Fi Aware”这一推断。

本机安装版本的苹果互联路径是：NFC 触发贴贴分享流程，BLE/Apple BLE 与 mDNS 承担设备发现或基础控制，高级连接使用 iPhone 热点或同一局域网 WLAN。小米的通用连接框架确实实现了 Android Wi‑Fi Aware provider，但当前 `iOSOneHopShare` 服务没有把 Aware 配置为这条路径的数据通道。

2026-07-18 的真机日志进一步确认：打开 iPhone 上的小米互联服务后，Android 通过 mDNS 发现 iPhone，并直接给出 `wifi_lan1 <> wifi_hotspot` 连接能力对；当时 Android 正连接到该 iPhone 的个人热点，P2P 与小米 MiWill 网络加速接口 `miw_oem0` 全程未启用。

因此，现阶段适合 Envoix 仿照的是“多发现源汇聚 + 多数据通道竞速/降级”的架构，而不是依赖小米的私有 Wi‑Fi 实现。

## 样本范围

- Android：`25060RK16C`（设备代号 `dali`），Android 16 / API 36，HyperOS `OS3.0.303.0.WONCNXM`
- Android 小米互传：`com.miui.mishare.connectivity` 4.13.7，versionCode 41307
- Android 天琴互联：`com.xiaomi.mi_connect_service` 5.1.175.10，versionCode 50011751
- iOS 小米互联服务：`com.xiaomi.hyperConnect` 3.0.0，bundle version 242
- MIShare APK SHA-256：`a71343f7ed685945167fe52394ead51b877a0e7b55f6d07cde50146a4aa2dd26`
- Lyra APK SHA-256：`1a8612dbe89a1d840d8567959567f5eaf7167f90cfad11bd67072d266051cff3`

反编译使用 JADX 1.5.5。JADX 对 MIShare 报告 9 个、对 Lyra 报告 31 个反编译错误，因此这里只把可交叉验证的服务常量、调用选择和运行时日志作为证据，不假定整个反编译结果完全正确。

## 分层逻辑

```text
NFC 贴碰 ─────────────┐
BLE / Apple BLE ──────┼─> DeviceInfo 聚合与去重 ─> 会话/能力协商
mDNS ─────────────────┘                              │
                                                     ├─ 基础通道：BLE 或 WLAN
                                                     └─ 高级通道：
                                                        Android -> P2P
                                                        iPhone  -> 热点或同 LAN
                                                               │
                                                               └─ 文件传输
```

这里的 NFC 不是完整数据通道，也不能简单描述为“完全负责一次配对”。它更像显式用户意图和贴贴分享流程的触发源；设备信息聚合、连接建立和文件传输仍由其他介质完成。

## 静态证据

### 1. OneHop 发现使用 Apple BLE、BLE 和 mDNS

MIShare 的 `i2.s0` 使用服务 ID `00370E2E`：

- 面向 Apple 的广播介质为 `BLE_APPLE | MDNS`（131072 与 4）。
- 小米侧广播为 `MDNS | BLE`（4 与 2）。
- 通用发现同样为 `MDNS | BLE`。
- `MediumType` 虽然定义了 `WIFI_AWARE=4096`，但当前 OneHop 的上述介质组合没有包含 4096。

代码还将一次动态发现限制在约 10 秒，广播约 35 秒，并对发现结果做短时间批处理，说明它的目标是低延迟、多来源去重，而不是绑定单一发现介质。

### 2. Apple 目标被明确映射到热点/同 LAN

`com.miui.mishare.connectivity.k1.v(...)` 根据设备类型判断是否为 Apple，Apple 目标选择 `iOSOneHopShare`，其他目标选择 `miOneHopShare`。

`s2.l0` 的服务映射直接描述：

- `miOneHopShare`：新版一碰传高级通道，P2P，面向 Android。
- `iOSOneHopShare`：新版一碰传高级通道；Apple 侧的 P2P 实际实现为热点，或使用 Apple 同局域网 WLAN。
- `miOneHopShareBasic`：新版一碰传基础通道，由天琴框架在 BLE 与 WLAN 等介质中选择。

这比仅从权限、API 类名或设备功能位推断连接方式更可靠，因为它是当前业务服务的实际映射。

### 3. 基础与高级连接并行，成功后取消另一路

客户端 `s2.z` 同时尝试：

- `miOneHopShareBasic` 基础连接；
- 远端声明的高级服务，Apple 设备即 `iOSOneHopShare`。

高级通道连接成功后会关闭基础通道。服务端 `s2.d0` 也同时监听两类连接并优先接受高级通道。这解释了关闭蓝牙仍可能正常传输：BLE 基础通道可以失败或不参与，热点/同 LAN 高级通道继续成功。

### 4. Wi‑Fi Aware 是通用框架能力，不等于本业务采用

Lyra 内确有完整的通用 Aware 实现：

- `AwareAttachManager` 与 `AwareDiscoveryManager` 获取 Android `wifiaware` 系统服务。
- `WifiCapabilityHelper` 反射调用 `WifiManager.isWifiAwareSupported()` 并设置能力位。
- `LinkCapability` 定义 `WIFI_AWARE_ONLY` 与 `WIFI_AWARE_PREFER` 策略。
- `AwareSysGovernor` 在 Wi‑Fi、SoftAP、P2P 共存条件允许时创建 Aware 连接。

但本机 `service list` 没有标准 `wifiaware` Binder 服务，系统 feature 查询为 false；更重要的是，当前 Apple OneHop 业务映射没有选择 Aware。由此只能得出“框架可在其他机型/业务上使用 Aware”，不能得出“本次苹果互联正在使用 Aware”。

`miw_oem0` 也不是 Aware 证据。可检索到的其他小米设备原始日志显示，该接口由 `MIWILL-HAL` 控制，并与双 Wi‑Fi就绪状态、网络加速开关、iptables/NAT 规则和接口地址清理处于同一控制链。它应按 MiWill 网络加速/流量聚合虚拟接口处理；即使测试中变为 UP，也不能据此归因为 NAN/Aware。

## 运行时证据

### 基线

Android 的主 WLAN 接口为 `wlan0`，地址位于 `172.20.10.0/28`，SSID 为本次连接的 iPhone 名称，默认网关为 iPhone 热点地址。基线候选专用接口状态为：

- `miw_oem0`：DOWN
- `p2p0`：DOWN / no carrier
- `p2p1`：DOWN

### 自动发现测试

自动化脚本同时将两端应用切到前台。Android 日志随后记录：

- `disc_type=mdns(4)`；
- 远端名称十六进制解码为 `iPhone`；
- 服务 ID 为 `270525`，对应通用 Lyra Share 服务；
- `conn_medium_pair=[wifi_lan1<>wifi_hotspot]`；
- 远端热点地址被标为 `direct_connected=true`。

整个 60 秒窗口内没有形成 Wi‑Fi Direct group，`miw_oem0`、`p2p0`、`p2p1` 均未变为 UP。该样本直接证明当前被动发现和候选连接路径是 mDNS + 热点 WLAN。

### NFC/OneHop 状态

Android 的“贴贴分享”页面明确要求双方亮屏解锁并开启 NFC、蓝牙和 WLAN，发送端需让两台手机背部 NFC 区域靠近。仅打开两端应用时，日志为：

```text
OneHopShare: nfc_share:false, apple_adv:false,
              mi_adv:false, mi_disc:false, sharing:false
```

因此，自动拉起应用只验证了后台 mDNS/LAN 发现；要触发 `00370E2E` OneHop 流程，仍需要一次真实 NFC 贴碰。

## 对 Envoix 的可复用设计

### 1. 统一发现结果，不统一底层实现

NFC、BLE、mDNS 和可选 Aware provider 都应输出同一种候选结构，例如稳定 peer ID、发现介质、可用端点、能力、认证材料引用和过期时间。会话层只合并候选，不感知各平台的扫描 API。

### 2. 把用户意图、发现和数据面拆开

- NFC：确认近距离用户意图，并交换最小会话引导信息。
- BLE/mDNS：补充或持续发现设备，完成端点与能力更新。
- WLAN/Aware/P2P：承载高吞吐数据。

这样即使 NFC 只触发一次、BLE 随后关闭，已有会话仍可在 WLAN 上完成传输。

### 3. 竞速高质量通道，并可靠取消输家

可仿照小米的“双连接”思路，但不要照搬私有服务名：为同一 session 同时启动低成本 fallback 与高吞吐 candidate；任一路完成认证并进入 ready 后，原子选定 winner，取消其他连接。连接回调必须携带 session ID、candidate ID 和明确状态，防止迟到回调覆盖 winner。

### 4. Aware 必须是可选 provider

普通第三方 Android 应用无法依赖厂商私有 MiWill/MiShare 权限。Envoix 应在运行时检测标准 Aware feature、系统服务和 attach 结果；任何一步不可用就回退到 mDNS + LAN 等公开能力，不把 OS 版本号当作支持依据。

### 5. 用结构化观测代替“关开关猜路径”

每次测试至少记录：发现介质、候选端点、最终 winner、接口名、地址族、路由、连接耗时、首字节时间、传输字节和失败原因。关闭蓝牙只能证明一次具体流程不依赖蓝牙，不能单独证明使用了哪种 Wi‑Fi 技术。

## 自动化工具

仓库脚本 `scripts/xiaomi-interconnect-path-probe.sh` 会只读采集 Android 全量日志、接口/地址/路由、Wi‑Fi Direct 状态、接口流量计数和两端进程快照；`--launch-apps` 可自动将两端小米互联切到前台。

示例：

```bash
ADB=/path/to/adb scripts/xiaomi-interconnect-path-probe.sh \
  --launch-apps \
  --seconds 90
```

它不会切换无线电、清除配对、清空日志或修改应用数据。日志可能包含设备名、局域网地址和网络标识，不应直接提交到公开仓库。

## 剩余验证矩阵

下一次需要在探针运行期间实际贴碰 NFC 并传输一个已知大小文件：

1. iPhone 热点开启、蓝牙开启：确认当前成功基线及 `wlan0` 字节增量。
2. iPhone 热点开启、蓝牙关闭：验证仍由 `wlan0` 热点路径完成。
3. 无共同 WLAN/热点、蓝牙关闭、NFC 贴碰：观察是否建立 P2P、标准 Aware data interface 或其他业务接口；`miw_oem0` 单独启用只说明 MiWill 网络加速参与。只有这一轮才有区分 Aware、P2P 和热点的价值。
4. 无共同 WLAN/热点、蓝牙开启：确认 BLE 是否只承担发现/控制，以及高级通道失败时的 fallback 行为。

第三轮若仍成功，需同时看到标准 Aware 网络、IPv6 地址/路由和明确的 NAN/Aware HAL 日志，才能把路径归因为 Aware；若 `p2p0/p2p1`、`miw_oem0` 或热点接口接管，则仍不能证明标准 Wi‑Fi Aware。
