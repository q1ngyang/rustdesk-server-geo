# rustdesk-server-geo

在官方 [rustdesk/rustdesk-server](https://github.com/rustdesk/rustdesk-server) OSS `hbbs` 上增加按连接双方国家/地区选择 `hbbr` 的轻量扩展。规则、MMDB 下载地址和更新周期全部通过环境变量配置，不需要维护自己的 Rust 分支。

> 这是非官方社区项目，与 RustDesk 或 MaxMind 没有隶属关系。镜像不会内置 GeoLite 数据库；使用者需要自行选择合法、可信的 MMDB 下载源，并遵守数据源许可证。

## 工作方式

- RustDesk 仍先尝试直连和 NAT 打洞。打洞成功后，数据不会经过 `hbbr`，Geo 选择也不会增加传输路径。
- 只有需要中继时，`hbbs` 才根据双方公网 IP 查询 Country MMDB，并从当前在线的 `hbbr` 中选择规则指定节点。
- 规则节点离线时自动尝试下一优先级；没有匹配规则、IP 无法定位、MMDB 不可用或配置错误时，回退到官方轮询逻辑。
- 所有客户端继续使用同一套 ID Server、API Server 和公钥配置。

> 客户端配置中的“中继服务器 / Relay Server”必须留空。RustDesk 客户端的静态 `relay-server` 选项优先于 `hbbs` 动态下发；一旦填写固定地址，Geo 选择将被客户端覆盖。统一配置仍然可以实现，只需统一填写 ID Server、API Server 和公钥，并统一留空 Relay Server。

## 镜像

```yaml
image: ghcr.io/q1ngyang/rustdesk-server-geo:latest
```

支持 `linux/amd64` 和 `linux/arm64`。镜像同时包含 `hbbs`、`hbbr` 和 `rustdesk-utils`，但 Geo 选择只作用于 `hbbs`；其他 `hbbr` 节点可以继续使用官方镜像。

完整示例见 [`examples/compose.yaml`](examples/compose.yaml) 和 [`examples/.env.example`](examples/.env.example)。宝塔面板部署时，将两份文件内容分别放入 Compose 和环境变量区域即可。

## 环境变量

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `GEO_RELAY_ENABLED` | `true` | 是否启用 Geo Relay 选择。关闭后完全使用官方逻辑。 |
| `GEO_RELAY_RULES` | 无 | 国家组合到 Relay 优先级的规则，启用时必填。 |
| `GEOIP_DB_URL` | 无 | Country MMDB 的 HTTPS 直链；为空时不自动下载。 |
| `GEOIP_DB_PATH` | `/root/geoip/GeoLite2-Country.mmdb` | 容器内 MMDB 路径。建议通过 `/root` 卷持久化。 |
| `GEOIP_UPDATE_INTERVAL` | `86400` | 自动更新周期，单位秒；`0` 表示关闭周期更新。 |
| `GEOIP_UPDATE_ON_START` | `true` | 容器启动时是否立即检查一次更新。 |
| `GEOIP_DOWNLOAD_TIMEOUT` | `180` | 单次下载最长秒数。 |
| `GEOIP_MIN_BYTES` | `65536` | 下载文件最小字节数，用于拦截错误页面。 |

### 规则语法

```dotenv
GEO_RELAY_RULES='CN-CN=relay-hk-1.example.com,relay-hk-2.example.com>relay-jp.example.com;CN-JP=relay-jp.example.com>relay-hk-1.example.com;CN-US=relay-us.example.com>relay-jp.example.com;DEFAULT=relay-jp.example.com>relay-hk-1.example.com>relay-us.example.com'
```

- `;` 分隔规则。
- `=` 左侧是两个 ISO 3166-1 alpha-2 国家代码；顺序不敏感，`JP-CN` 与 `CN-JP` 相同。
- `,` 表示同一优先级内轮询。
- `>` 表示下一故障转移优先级。
- `DEFAULT` 处理没有专用规则或无法定位的连接。
- 节点名称必须与 `RELAY_SERVERS` 中的值完全对应，比较时不区分大小写；如其中包含端口，规则也要包含相同端口。

例如，连接双方都在中国时优先香港节点；中国与日本之间优先日本节点；中国与美国之间优先美国节点。这里按“双方位置组合”选择，不是仅按发起方选择，因此同一客户端配置可以覆盖中国、日本和美国。

## MMDB 自动更新

容器启动脚本会把 `GEOIP_DB_URL` 下载到临时文件，检查最小体积和 MaxMind DB 标记后原子替换现有文件。下载失败、文件异常或内容未变化时保留旧文件。周期更新成功后会通知 `hbbs` 热加载，无需重启容器。

示例使用 [P3TERX/GeoLite.mmdb](https://github.com/P3TERX/GeoLite.mmdb) 整理的 Country 数据库直链：

```dotenv
GEOIP_DB_URL=https://github.com/P3TERX/GeoLite.mmdb/raw/download/GeoLite2-Country.mmdb
```

该地址是第三方服务，稳定性、准确性和许可证合规由部署者自行评估。也可以替换为自己的 MaxMind 下载流程、对象存储或其他兼容的 Country MMDB 直链。

## 多节点部署要点

1. 中心服务器运行本镜像的 `hbbs`，通过 `RELAY_SERVERS` 注册所有可用 `hbbr`。
2. 香港、日本、美国等中继服务器分别运行 `hbbr`，可以直接使用官方 `rustdesk/rustdesk-server` 镜像。
3. 所有节点使用相同的 RustDesk 密钥；只开放实际需要的 RustDesk TCP/UDP 端口，Cloudflare 记录使用“仅 DNS”。
4. `hbbs` 会先剔除健康检查失败的中继节点，Geo 规则只会从剩余在线节点中选择。

可在中心容器内测试两个公网 IP 对应的选择结果：

```sh
docker exec rustdesk-hbbs sh -c "printf 'test-geo 1.1.1.1 8.8.8.8\n' | nc -w 2 127.0.0.1 21115"
```

## 自动同步和构建

GitHub Actions 每天检查一次官方最新正式 Release，也支持手动指定标签、分支或提交：

1. 克隆官方源码及子模块。
2. 严格匹配补丁锚点；上游结构改变时立即失败，避免生成行为不确定的镜像。
3. 运行 Geo 规则单元测试。
4. 交叉编译 `amd64`、`arm64` 的 musl 静态二进制。
5. 发布版本标签和 `latest` 多架构镜像到 GHCR。

扩展行为变更时应增加 [`PATCH_VERSION`](PATCH_VERSION)，以生成新的 `上游版本-geo.补丁版本` 标签。定时任务发现该版本已发布时会跳过重复构建。

## 与上游的关系

本仓库只保存补丁层、构建脚本和部署示例，不复制或长期维护完整上游源码。构建产物基于 RustDesk Server OSS，继续遵循上游的 AGPL-3.0 许可证。本项目自身也使用 AGPL-3.0。
