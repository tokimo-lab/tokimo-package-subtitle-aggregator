# Subtitle Aggregator — 聚合多源字幕搜索与下载

聚合 **51 个字幕源**，并发搜索、统一返回结果，支持按关键词、IMDb ID、TMDb ID、文件哈希等多种方式检索。

## 目录

- [快速开始](#快速开始)
- [搜索请求参数说明](#搜索请求参数说明)
- [环境变量 / 凭证配置](#环境变量--凭证配置)
- [字幕源总览](#字幕源总览)
  - [按搜索方式分类](#按搜索方式分类)
  - [完整字幕源详情表](#完整字幕源详情表)
- [搜索结果格式](#搜索结果格式)
- [下载请求格式](#下载请求格式)

---

## 快速开始

```bash
# 编译
cargo build --release

# 搜索（默认搜索 "Inception"）
cargo run --release

# 搜索指定关键词
cargo run --release -- "盗梦空间"

# 带日志级别
RUST_LOG=info cargo run --release -- "Inception"
```

---

## 搜索请求参数说明

```rust
SubtitleSearchRequest {
    query: Option<String>,         // 关键词搜索（片名 / 剧名）
    imdb_id: Option<String>,       // IMDb ID，如 "tt1375666"
    tmdb_id: Option<String>,       // TMDb ID，如 "27205"
    languages: Option<Vec<String>>,// 语言过滤，如 ["zh-CN", "en"]
    file_hash: Option<String>,     // 文件哈希（射手网 / 迅雷 / TheSubDB 等）
    file_size: Option<u64>,        // 文件大小（字节），BSPlayer / Napisy24 需要
}
```

> **注意**：不同字幕源使用不同的参数组合。如果某个必需参数缺失，该字幕源会被静默跳过（日志中可见 WARN）。

---

## 环境变量 / 凭证配置

以下字幕源需要或支持通过环境变量配置 API Key / 账号密码：

| 环境变量 | 字幕源 | 是否必需 | 说明 |
|---|---|---|---|
| `OPENSUBTITLES_API_KEY` | OpenSubtitles | **必需** | [申请地址](https://www.opensubtitles.com/consumers) |
| `JIMAKU_API_KEY` | Jimaku | **必需** | [jimaku.cc](https://jimaku.cc) API Key |
| `BETASERIES_API_KEY` | BetaSeries | **必需** | [betaseries.com](https://www.betaseries.com/api/) API Key |
| `SUBX_API_KEY` | SubX | **必需** | SubX API Key |
| `KTUVIT_USER` / `KTUVIT_PASS` | Ktuvit | **必需** | ktuvit.me 账号密码 |
| `TITLOVI_USER` / `TITLOVI_PASS` | Titlovi | **必需** | titlovi.com 账号密码 |
| `TITULKY_USER` / `TITULKY_PASS` | Titulky | **必需** | premium.titulky.com 账号密码 |
| `LEGENDASNET_USER` / `LEGENDASNET_PASS` | LegendasNet | **必需** | legendas.net 账号密码 |
| `SUBDL_API_KEY` | SubDL | 可选 | 不提供也可用，有 Key 可提高配额 |
| `SUBSOURCE_API_KEY` | SubSource | 可选 | 不提供时功能受限 |
| `ADDIC7ED_USER` / `ADDIC7ED_PASS` | Addic7ed | 可选 | 不提供也可匿名搜索 |
| `LEGENDASDIVX_USER` / `LEGENDASDIVX_PASS` | LegendasDivx | 可选 | 不提供也可匿名搜索 |
| `NAPISY24_USER` / `NAPISY24_PASS` | Napisy24 | 可选 | 不提供也可匿名搜索 |

---

## 字幕源总览

### 按搜索方式分类

#### 🔤 关键词搜索（`query`）— 38 个源

大部分字幕源支持关键词搜索，传入 `query` 即可。

| 字幕源 | 主要语言 | `languages` 过滤 | 备注 |
|---|---|---|---|
| **assrt** | 🇨🇳 中文 | ✗ | 中国最大字幕搜索聚合，也支持 `file_hash` 回退 |
| **zimuku** | 🇨🇳 中文 | ✗ | 字幕库，也可选传 `imdb_id` |
| **subf2m** | 🌍 多语言 | ✅ | 全球最大字幕源之一，30+ 语言 |
| **podnapisi** | 🌍 多语言 | ✅ | 支持中/英/日/韩等，也可选传 `imdb_id` |
| **addic7ed** | 🌍 多语言 | ✅ | 英语/法语/西语等，TV 剧集为主 |
| **gestdown** | 🌍 多语言 | ✅ | TV 剧集 API，法语/英语等 24 种语言 |
| **tvsubtitles** | 🌍 多语言 | ✅ | TV 剧集，25 种语言 |
| **animetosho** | 🌍 多语言 | ✅ | 动漫字幕，18 种语言 |
| **subdl** | 🌍 多语言 | ✅ | 也支持 `imdb_id` / `tmdb_id`，可选 API Key |
| **subsource** | 🌍 多语言 | ✅ | 60+ 语言，也可选传 `imdb_id`，可选 API Key |
| **yify** | 🌍 多语言 | ✅ | YIFY 字幕站，也支持 `imdb_id` 优先查找 |
| **supersubtitles** | 🇭🇺 匈牙利/英语 | ✗ | feliratok.eu，支持 S##E## 模式 |
| **hosszupuska** | 🇭🇺 匈牙利/英语 | ✗ | TV 剧集，**query 需含 S##E## 格式** |
| **animekalesi** | 🇹🇷 土耳其 | ✗ | 土耳其动漫字幕 |
| **animesubinfo** | 🇹🇷 土耳其 | ✗ | 土耳其动漫字幕 |
| **soustitreseu** | 🇫🇷 法语 | ✗ | 法语字幕 |
| **subsynchro** | 🇫🇷 法语 | ✗ | 法语电影字幕 |
| **legendasdivx** | 🇵🇹 葡萄牙语 | ✅ | 葡萄牙/巴西字幕，可选登录 |
| **greeksubtitles** | 🇬🇷 希腊语 | ✅ | 希腊字幕 |
| **subs4free** | 🇬🇷 希腊语 | ✅ | 希腊字幕 |
| **subs4series** | 🇬🇷 希腊语 | ✅ | 希腊 TV 剧集，**query 需含 S##E## 格式** |
| **xsubs** | 🇬🇷 希腊语 | ✅ | 希腊 TV 剧集，**query 需含 S##E## 格式** |
| **subscenter** | 🇮🇱 希伯来语 | ✅ | 以色列字幕站 |
| **wizdom** | 🇮🇱 希伯来语 | ✗ | 也支持 `imdb_id`，内部调 TMDB API |
| **subtitrarinoi** | 🇷🇴 罗马尼亚语 | ✗ | 也可选传 `imdb_id` |
| **titrari** | 🇷🇴 罗马尼亚语 | ✅ | 也可选传 `imdb_id` |
| **regielive** | 🇷🇴 罗马尼亚语 | ✗ | 内置 API Key |
| **subssabbz** | 🇧🇬 保加利亚语 | ✅ | sab.bz 字幕 |
| **subsunacs** | 🇧🇬 保加利亚语 | ✅ | subsunacs.net |
| **nekur** | 🇱🇻 拉脱维亚语 | ✗ | nekur.net |
| **subtitriid** | 🇪🇪 爱沙尼亚语 | ✗ | subtitri.do.am |
| **subtitulamostv** | 🇪🇸 西语/英语 | ✗ | TV 剧集 |
| **subtis** | 🇪🇸 西班牙语 | ✗ | 也支持 `file_hash` / `file_size` 优先 |
| **subx** | 🇭🇺 匈牙利语 | ✗ | **需要 SUBX_API_KEY**，也可选传 `imdb_id` |
| **ktuvit** | 🇮🇱 希伯来语 | ✅ | **需要 KTUVIT_USER/PASS**，也可选传 `imdb_id` |
| **titlovi** | 🇭🇷 克罗地亚/塞尔维亚 | ✅ | **需要 TITLOVI_USER/PASS**，也可选传 `imdb_id` |
| **legendasnet** | 🇧🇷 葡萄牙语(巴西) | ✅ | **需要 LEGENDASNET_USER/PASS**，也可选传 `imdb_id` |
| **betaseries** | 🇫🇷 法语/英语 | ✅ | **需要 BETASERIES_API_KEY**，TV 剧集 |

#### 🆔 IMDb ID 搜索（`imdb_id`）— 5 个源（仅支持 IMDb ID）

以下字幕源 **必须** 提供 `imdb_id`，不支持关键词搜索：

| 字幕源 | 主要语言 | `languages` 过滤 | 网站 |
|---|---|---|---|
| **greeksubs** | 🇬🇷 希腊语/英语 | ✅ | greeksubs.net |
| **turkcealtyazi** | 🇹🇷 土耳其语/英语 | ✅ | turkcealtyazi.org |
| **yavkanet** | 🇧🇬 保加利亚语 | ✗ | yavka.net |
| **subsro** | 🇷🇴 罗马尼亚语/英语 | ✅ | subs.ro |
| **titulky** | 🇨🇿 捷克/斯洛伐克 | ✅ | premium.titulky.com，**需要 TITULKY_USER/PASS** |

#### 🆔 TMDb ID 搜索（`tmdb_id`）

| 字幕源 | 主要语言 | 备注 |
|---|---|---|
| **jimaku** | 🇯🇵 日语/英语 | 优先 `tmdb_id`，回退 `query`，**需要 JIMAKU_API_KEY** |
| **opensubtitles** | 🌍 多语言 | 支持 `query` / `imdb_id` / `tmdb_id`，**需要 API Key** |
| **subdl** | 🌍 多语言 | 支持 `query` / `imdb_id` / `tmdb_id` |

#### #️⃣ 文件哈希搜索（`file_hash`）— 6 个源

以下字幕源基于视频文件哈希精确匹配，**必须** 提供 `file_hash`：

| 字幕源 | 主要语言 | 还需 `file_size` | 哈希算法 | 网站 |
|---|---|---|---|---|
| **shooter** | 🇨🇳 中文 | ✗ | 射手网专用哈希 | shooter.cn |
| **xunlei** | 🇨🇳 中文 | ✗ | 迅雷 CID 哈希 | sub.xmp.sandai.net |
| **thesubdb** | 🌍 多语言 | ✗ | 文件首尾各 64KB 的 MD5 | thesubdb.com |
| **bsplayer** | 🌍 多语言 | **✅ 必需** | BSPlayer 专用哈希 | bsplayer-subtitles.com |
| **napiprojekt** | 🇵🇱 波兰语 | ✗ | MD5 哈希 | napiprojekt.pl |
| **napisy24** | 🇵🇱 波兰语 | **✅ 必需** | MD5 哈希 | napisy24.pl |

> **提示**：`subtis`（西班牙语）和 `assrt`（中文）也支持 `file_hash` 但非必需，会回退到 `query` 搜索。

---

### 完整字幕源详情表

| # | 字幕源名称 | 搜索方式 | 必需参数 | 可选参数 | 凭证 | 主要语言 | 网站 |
|---|---|---|---|---|---|---|---|
| 1 | **assrt** | query / file_hash | query | file_hash | — | 中文 | assrt.net |
| 2 | **zimuku** | query | query | imdb_id | — | 中文 | zimuku.cn |
| 3 | **shooter** | file_hash | file_hash | — | — | 中文 | shooter.cn |
| 4 | **xunlei** | file_hash | file_hash | — | — | 中文 | sub.xmp.sandai.net |
| 5 | **opensubtitles** | query / imdb_id / tmdb_id | query 或 imdb_id | tmdb_id, languages | `OPENSUBTITLES_API_KEY` | 多语言 | opensubtitles.com |
| 6 | **subdl** | query / imdb_id / tmdb_id | query 或 imdb_id 或 tmdb_id | languages | `SUBDL_API_KEY`(可选) | 多语言 | subdl.com |
| 7 | **subf2m** | query | query | languages | — | 多语言 | subf2m.co |
| 8 | **podnapisi** | query | query | imdb_id, languages | — | 多语言 | podnapisi.net |
| 9 | **yify** | query / imdb_id | query 或 imdb_id | languages | — | 多语言 | yifysubtitles.ch |
| 10 | **addic7ed** | query | query | languages | `ADDIC7ED_USER/PASS`(可选) | 多语言 | addic7ed.com |
| 11 | **gestdown** | query | query | languages | — | 多语言(TV) | gestdown.info |
| 12 | **tvsubtitles** | query | query | languages | — | 多语言(TV) | tvsubtitles.net |
| 13 | **animetosho** | query | query | languages | — | 多语言(动漫) | animetosho.org |
| 14 | **subsource** | query | query | imdb_id, languages | `SUBSOURCE_API_KEY`(可选) | 多语言 | subsource.net |
| 15 | **bsplayer** | file_hash | file_hash + file_size | imdb_id, languages | — | 多语言 | bsplayer-subtitles.com |
| 16 | **thesubdb** | file_hash | file_hash | — | — | 多语言 | thesubdb.com |
| 17 | **jimaku** | tmdb_id / query | tmdb_id 或 query | — | `JIMAKU_API_KEY` | 日语/英语 | jimaku.cc |
| 18 | **betaseries** | query / imdb_id | query 或 imdb_id | languages | `BETASERIES_API_KEY` | 法语/英语(TV) | betaseries.com |
| 19 | **soustitreseu** | query | query | — | — | 法语 | sous-titres.eu |
| 20 | **subsynchro** | query | query | — | — | 法语 | subsynchro.com |
| 21 | **legendasdivx** | query | query | languages | `LEGENDASDIVX_USER/PASS`(可选) | 葡萄牙语 | legendasdivx.pt |
| 22 | **legendasnet** | query / imdb_id | query 或 imdb_id | languages | `LEGENDASNET_USER/PASS` | 葡萄牙语(巴西) | legendas.net |
| 23 | **greeksubs** | imdb_id | imdb_id | languages | — | 希腊语/英语 | greeksubs.net |
| 24 | **greeksubtitles** | query | query | languages | — | 希腊语 | greek-subtitles.com |
| 25 | **subs4free** | query | query | languages | — | 希腊语 | subs4free.info |
| 26 | **subs4series** | query (S##E##) | query (含 S##E##) | languages | — | 希腊语(TV) | subs4series.com |
| 27 | **xsubs** | query (S##E##) | query (含 S##E##) | languages | — | 希腊语(TV) | xsubs.tv |
| 28 | **subscenter** | query | query | languages | — | 希伯来语 | subscenter.info |
| 29 | **wizdom** | query / imdb_id | query 或 imdb_id | — | — | 希伯来语 | wizdom.xyz |
| 30 | **ktuvit** | query | query | imdb_id, languages | `KTUVIT_USER/PASS` | 希伯来语 | ktuvit.me |
| 31 | **turkcealtyazi** | imdb_id | imdb_id | languages | — | 土耳其语/英语 | turkcealtyazi.org |
| 32 | **animekalesi** | query | query | — | — | 土耳其语(动漫) | animekalesi.com |
| 33 | **animesubinfo** | query | query | — | — | 土耳其语(动漫) | animesub.info |
| 34 | **subtitrarinoi** | query / imdb_id | query 或 imdb_id | — | — | 罗马尼亚语 | subtitrari-noi.ro |
| 35 | **titrari** | query / imdb_id | query 或 imdb_id | languages | — | 罗马尼亚语 | titrari.ro |
| 36 | **regielive** | query | query | — | — | 罗马尼亚语 | regielive.ro |
| 37 | **subsro** | imdb_id | imdb_id | languages | — | 罗马尼亚语/英语 | subs.ro |
| 38 | **subssabbz** | query | query | languages | — | 保加利亚语/英语 | subs.sab.bz |
| 39 | **subsunacs** | query | query | languages | — | 保加利亚语/英语 | subsunacs.net |
| 40 | **yavkanet** | imdb_id | imdb_id | — | — | 保加利亚语 | yavka.net |
| 41 | **titlovi** | query | query | imdb_id, languages | `TITLOVI_USER/PASS` | 克罗地亚/塞尔维亚 | titlovi.com |
| 42 | **titulky** | imdb_id | imdb_id | languages | `TITULKY_USER/PASS` | 捷克/斯洛伐克 | titulky.com |
| 43 | **supersubtitles** | query | query (±S##E##) | — | — | 匈牙利语/英语 | feliratok.eu |
| 44 | **hosszupuska** | query (S##E##) | query (含 S##E##) | — | — | 匈牙利语/英语(TV) | hosszupuskasub.com |
| 45 | **subx** | query / imdb_id | query 或 imdb_id | — | `SUBX_API_KEY` | 匈牙利语 | subx-api.duckdns.org |
| 46 | **napiprojekt** | file_hash | file_hash | — | — | 波兰语 | napiprojekt.pl |
| 47 | **napisy24** | file_hash | file_hash + file_size | query | `NAPISY24_USER/PASS`(可选) | 波兰语 | napisy24.pl |
| 48 | **nekur** | query | query | — | — | 拉脱维亚语 | nekur.net |
| 49 | **subtitriid** | query | query | — | — | 爱沙尼亚语 | subtitri.do.am |
| 50 | **subtitulamostv** | query | query | — | — | 西语/英语(TV) | subtitulamos.tv |
| 51 | **subtis** | file_hash / query | file_hash 或 file_size 或 query | — | — | 西班牙语 | subt.is |

---

## 搜索结果格式

每个字幕源返回统一的 `SubtitleSearchResult`：

```json
{
  "id": "字幕唯一标识",
  "name": "字幕文件名 / 标题",
  "language": "zh-CN",
  "languageName": "简体中文",
  "format": "srt",
  "provider": "assrt",
  "detailPath": "/optional/detail/url",
  "downloadPath": "/optional/download/url",
  "downloadCount": 12345,
  "rating": 9.5,
  "movieName": "盗梦空间",
  "releaseGroup": "CHD"
}
```

### 语言代码映射

| 代码 | 语言 |
|---|---|
| `zh-CN` | 简体中文 |
| `zh-TW` | 繁体中文 |
| `zh` | 中文(双语/未区分) |
| `en` | 英语 |
| `ja` | 日语 |
| `ko` | 韩语 |

---

## 下载请求格式

```json
{
  "subtitleId": "字幕ID（来自搜索结果的 id）",
  "detailPath": "来自搜索结果",
  "downloadPath": "来自搜索结果",
  "language": "zh-CN",
  "format": "srt",
  "name": "字幕名称",
  "provider": "assrt"
}
```

> `provider` 字段用于路由到对应的字幕源进行下载，默认为 `"assrt"`。

---

## 实测结果参考

以下为搜索 `"Inception"` 时各字幕源的实测状态（`query="Inception"`, `languages=["zh-CN","zh","en"]`，无 `imdb_id` / `file_hash`）：

### ✅ 成功返回结果

| 字幕源 | 结果数 | 说明 |
|---|---|---|
| subf2m | 154 | 全球最大字幕源，结果最多 |
| assrt | 11 | 中文字幕首选 |
| subsunacs | 10 | 保加利亚语字幕 |
| subtitrarinoi | 9 | 罗马尼亚语字幕 |
| subssabbz | 5 | 保加利亚语字幕 |
| soustitreseu | 1 | 法语字幕 |
| subtitriid | 1 | 爱沙尼亚语字幕 |

### ⏸️ 搜索成功但无结果（0 条）

gestdown, subtitulamostv, subtis, animetosho, nekur, animesubinfo, animekalesi, supersubtitles, addic7ed, titrari, legendasdivx, wizdom

### ⚠️ 跳过（缺少必需参数）

| 字幕源 | 原因 |
|---|---|
| bsplayer | 需要 `file_hash` |
| napiprojekt | 需要 `file_hash` |
| napisy24 | 需要 `file_hash` |
| shooter | 需要 `file_hash` |
| xunlei | 需要 `file_hash` |
| thesubdb | 需要 `file_hash` |
| greeksubs | 需要 `imdb_id` |
| turkcealtyazi | 需要 `imdb_id` |
| subsro | 需要 `imdb_id` |
| yavkanet | 需要 `imdb_id` |
| subs4series | 需要 query 含 S##E## 格式 |
| xsubs | 需要 query 含 S##E## 格式 |

### 🔒 跳过（缺少凭证）

| 字幕源 | 缺少的环境变量 |
|---|---|
| betaseries | `BETASERIES_API_KEY` |
| ktuvit | `KTUVIT_USER` / `KTUVIT_PASS` |
| legendasnet | `LEGENDASNET_USER` / `LEGENDASNET_PASS` |
| subx | `SUBX_API_KEY` |

---

## 架构说明

```
src/
├── main.rs          # CLI 入口，注册所有 provider
├── lib.rs           # 库入口
├── models.rs        # 数据模型（请求/响应）
├── aggregator.rs    # 聚合器：并发搜索 + 下载路由
├── archive.rs       # 压缩包解压（ZIP/RAR/7Z/XZ）
└── providers/       # 51 个字幕源实现
    ├── mod.rs       # SubtitleProvider trait 定义
    ├── assrt.rs
    ├── opensubtitles.rs
    └── ...
```

- 所有 provider 实现 `SubtitleProvider` trait（`search` + `download`）
- 搜索时所有 provider **并发执行**，单个 provider 超时 30 秒自动取消
- 搜索失败的 provider 不影响其他 provider 的结果
