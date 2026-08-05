# rustdesk-server-geo

在官方 [rustdesk/rustdesk-server](https://github.com/rustdesk/rustdesk-server) OSS `hbbs` 上增加按连接双方 Country、City、ASN 信息选择 `hbbr` 的扩展。规则、三个 MMDB 下载地址和更新周期全部通过环境变量配置，不需要维护完整 RustDesk Server 分支。

> 这是非官方社区项目，与 RustDesk、MaxMind 或 MMDB 镜像提供方没有隶属关系。镜像不会内置 GeoLite2 数据库；部署者需要自行选择合法、可信的数据源并遵守其许可证。

## 工作方式

- RustDesk 仍然先尝试直连和 NAT 打洞；打洞成功后数据不经过 `hbbr`。
- 只有需要中继时，`hbbs` 才查询双方公网 IP，并从上到下检查有序规则。
- 首条匹配且包含在线节点的规则生效；同级节点轮询，节点不可用时尝试下一 tier。
- 某条规则匹配但其所有节点都离线时，继续检查下一条规则。
- 没有可用规则时回退到官方在线 Relay 轮询逻辑。
- 所有客户端继续使用同一套 ID Server、API Server 和公钥配置。

> 客户端配置中的“中继服务器 / Relay Server”必须留空。RustDesk 客户端的静态 `relay-server` 选项优先于 `hbbs` 动态下发；填写固定地址会绕过本项目的选择逻辑。

## 镜像

```yaml
image: ghcr.io/q1ngyang/rustdesk-server-geo:latest
```

支持 `linux/amd64` 和 `linux/arm64`。镜像包含 `hbbs`、`hbbr` 和 `rustdesk-utils`，但 Geo 选择只作用于中心 `hbbs`；其他中继节点可以继续运行官方 `rustdesk/rustdesk-server` 镜像。

完整配置见 [`examples/compose.yaml`](examples/compose.yaml) 和 [`examples/.env.example`](examples/.env.example)。

## 环境变量

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `GEO_RELAY_ENABLED` | `true` | 是否启用 Geo Relay 选择。 |
| `GEO_RELAY_RULES` | 无 | YAML v2 多行有序规则；启用时必填。 |
| `GEOIP_COUNTRY_DB_URL` | 无 | GeoLite2 Country MMDB 直链。 |
| `GEOIP_CITY_DB_URL` | 无 | GeoLite2 City MMDB 直链；不使用城市规则时可留空。 |
| `GEOIP_ASN_DB_URL` | 无 | GeoLite2 ASN MMDB 直链；不使用 ASN 规则时可留空。 |
| `GEOIP_COUNTRY_DB_PATH` | `/root/geoip/GeoLite2-Country.mmdb` | Country 数据库持久化路径。 |
| `GEOIP_CITY_DB_PATH` | `/root/geoip/GeoLite2-City.mmdb` | City 数据库持久化路径。 |
| `GEOIP_ASN_DB_PATH` | `/root/geoip/GeoLite2-ASN.mmdb` | ASN 数据库持久化路径。 |
| `GEOIP_UPDATE_INTERVAL` | `168h` | 更新周期；支持 `m`、`h`、`d`、`s`，`0` 关闭。 |
| `GEOIP_UPDATE_ON_START` | `true` | 启动时检查缺失或过期数据库。 |
| `GEOIP_FORCE_UPDATE_ON_START` | `false` | 是否忽略文件年龄并在每次启动时强制下载。 |
| `GEOIP_DOWNLOAD_TIMEOUT` | `600` | 每个数据库的单次下载超时秒数。 |
| `GEOIP_MIN_BYTES` | `65536` | 最小文件体积校验。 |

旧版 `GEOIP_DB_URL`、`GEOIP_DB_PATH` 仍作为 Country 变量的兼容别名；旧的一行式 `CN-CN=...;DEFAULT=...` 规则也仍可解析。建议迁移到 YAML v2。

## YAML v2 规则

`.env` 支持单引号包裹的多行值，因此每个国家组合、城市或运营商规则都可以独立成块：

```dotenv
GEO_RELAY_RULES='version: 2
rules:
  - name: "上海电信到东京"
    match:
      client_a:
        all:
          - country: CN
          - city: [Shanghai, 上海]
          - any:
              - asn: 4134
              - asn_org_contains: "China Telecom"
      client_b:
        all:
          - country: JP
          - city: [Tokyo, 東京]
    relay_tiers:
      - [relay-tokyo-1.example.com, relay-tokyo-2.example.com]
      - [relay-osaka.example.com]

  - name: "CN-JP"
    match:
      client_a: { country: CN }
      client_b: { country: JP }
    relay_tiers:
      - [relay-jp.example.com]
      - [relay-cn.example.com]

  - name: "DEFAULT"
    match: {}
    relay_tiers:
      - [relay-jp.example.com]
      - [relay-us.example.com]
'
```

### 匹配字段

每个 `client_a` / `client_b` 支持：

| 字段 | 数据库 | 含义 |
| --- | --- | --- |
| `continent` | Country 或 City | 两位洲代码，例如 `AS`、`NA`。 |
| `country` | Country 或 City | ISO 3166-1 两位国家代码。 |
| `subdivision` | City | 省、州等 ISO 3166-2 子区域代码或数据库名称。 |
| `city` | City | 城市名称，支持数据库中的英文、简体中文、日文等名称。 |
| `city_geoname_id` | City | GeoNames 城市 ID；比文本名称更稳定。 |
| `asn` | ASN | 自治系统编号，例如 `4134`。 |
| `asn_org_contains` | ASN | 运营商名称的忽略大小写包含匹配。 |

同一字段写多个值表示“或”，同一个 matcher 中不同字段表示“且”。`all`、`any`、`not` 可以递归嵌套：

```yaml
client_a:
  all:
    - country: CN
    - any:
        - city: Shanghai
        - asn: [4134, 4809]
  not:
    asn_org_contains: "China Mobile"
```

规则默认 `symmetric: true`，即 `client_a/client_b` 交换后仍可匹配。需要方向敏感时可在规则中设置 `symmetric: false`。

`relay_tiers` 从上到下表示故障转移优先级；同一行数组中的多个在线节点轮询。节点名称必须与传给 `hbbs -r/--relay-servers` 的列表完全对应，如包含端口，规则也必须包含相同端口。不要使用下划线形式的 `RELAY_SERVERS`，OSS `hbbs` 不会读取它；如改用环境变量，官方名称是带连字符的 `RELAY-SERVERS`。

## MMDB 自动更新与内存

默认更新周期为 `168h`，即每周一次。还可以使用 `30m`、`12h`、`7d`；为兼容旧配置，纯数字仍按秒解释。

启动检查会根据持久化文件修改时间判断是否过期，不会因为容器重启而重复下载。下载器会写入临时文件，检查体积及 MaxMind DB 标记，再原子替换；失败时保留旧文件，成功后通知 `hbbs` 热加载。

三个数据库使用 mmap 按需分页，避免把约 80–90 MB 数据重复复制到 Rust 堆内存。原子替换不会修改仍被旧 Reader 映射的 inode，热加载完成后旧映射才释放。

示例使用第三方 [P3TERX/GeoLite.mmdb](https://github.com/P3TERX/GeoLite.mmdb) 的三个直链：

```dotenv
GEOIP_COUNTRY_DB_URL=https://github.com/P3TERX/GeoLite.mmdb/raw/download/GeoLite2-Country.mmdb
GEOIP_CITY_DB_URL=https://github.com/P3TERX/GeoLite.mmdb/raw/download/GeoLite2-City.mmdb
GEOIP_ASN_DB_URL=https://github.com/P3TERX/GeoLite.mmdb/raw/download/GeoLite2-ASN.mmdb
```

这些是第三方服务，稳定性、准确性和许可证合规由部署者自行评估。

## 关于延迟和丢包路由

当前 RustDesk OSS 协议没有把客户端到各 `hbbr` 的延迟、丢包或中继失败结果上报给 `hbbs`。`hbbs` 只负责在连接前下发一个 Relay 地址，连接之后也收不到质量反馈，因此仅修改 OSS `hbbs` 无法真实完成“尝试节点、按质量继续回退、再选择历史最优”的闭环。

本项目不会使用 `hbbs -> hbbr` 的 ping 冒充客户端链路质量，因为中心服务器到节点的延迟不能代表中国、日本、美国客户端到节点的体验。可行扩展有两类：

1. 部署在代表性地区/ASN 内的外部探针，向 `hbbs` 提供带有效期的分区质量矩阵；不修改 RustDesk 客户端，但属于区域近似值。
2. 扩展 RustDesk 客户端和协议，由双方客户端测量候选 `hbbr` 并回报结果；数据最准确，但会显著增加客户端发布和上游同步维护成本。

在没有可信质量数据源时，处理顺序保持为：有序 Geo 规则 → 官方在线 Relay 轮询。

## 多节点部署

1. 中心服务器运行本镜像的 `hbbs`，通过 `-r`/`--relay-servers` 传入全部中继节点。
2. 中国、日本、美国等服务器分别运行 `hbbr`，可以继续使用官方镜像。
3. 所有节点使用同一套 RustDesk 密钥；Cloudflare 记录使用“仅 DNS”。
4. `hbbs` 先剔除健康检查失败的中继，规则只从剩余在线节点中选择。

测试两个公网 IP 的最终选择结果：

```sh
docker exec rustdesk-hbbs sh -c "printf 'test-geo 1.1.1.1 8.8.8.8\n' | nc -w 2 127.0.0.1 21115"
```

## 自动同步和构建

GitHub Actions 每天检查官方最新正式 Release，也支持手动指定标签、分支或提交：

1. 克隆官方源码及子模块。
2. 严格匹配补丁锚点；上游结构改变时立即失败。
3. 运行 YAML、嵌套匹配和旧规则兼容性单元测试。
4. 交叉编译 `amd64`、`arm64` 的 musl 静态二进制。
5. 发布版本标签和 `latest` 多架构镜像到 GHCR。

扩展行为变化时增加 [`PATCH_VERSION`](PATCH_VERSION)，生成新的 `上游版本-geo.补丁版本` 标签。

## 与上游的关系

本仓库只保存补丁层、构建脚本和部署示例，不复制或长期维护完整上游源码。构建产物和本项目继续遵循 AGPL-3.0。
