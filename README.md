# usuyo-fetch/weibo

微博的获取插件，供 [usuyo](https://github.com/usuyo/usuyo) 调用：按链接取回单条微博。

| 功能 | 实现方式（方法 id） | 账号 | 风险层级 | 状态 |
|---|---|---|---|---|
| 按链接取单条内容（文字、图片、视频、转发的那一条） | 公开微博页的数据接口（`status`） | 访客身份（在 usuyo 里取一次，不是账号） | `public-web`，默认开 | 已实现（2026-11-16 用访客身份对着真的微博取过一条带视频的公开微博，文字和视频都取到；长文、图片、转发只在本机的假服务器上测过） |
| 视频文件 | 随 `status` 一起取微博里的视频 | 同 `status` | 同 `status` | 已实现（验到哪同 `status`）；只取微博自己带的视频，别处的视频页未实现 |
| 列出我的收藏、赞过 | 浏览器脚本（`userscript`） | 使用者自己的浏览器里的登录 | `browser-session`，需要登录，登录后开 | 脚本已做（没在真浏览器里跑过） |
| 列出我的收藏 | 登录进 usuyo（`favorites`） | 登录进 usuyo | `session-api`，需要登录，登录后开 | 未实现 |

## 方法

### `status`：公开微博页的数据接口

读公开的 `ajax/statuses/show`（长文再读 `ajax/statuses/longtext`），取得单条微博：文字（长文）、作者、发布时间、计数、来源、图片、视频、转发的那一条、原始回答。要一份访客身份：usuyo 打开一次微博首页、不登录，存下页面自己生成的 cookie，经 `USUYO_VISITOR` 交给插件；没有就回 `visitor-required`。访客身份只发给接口的主机，图片和视频的请求不带 cookie。风险记录：访客路径只见到 IP 层的限制，微博协议 4.5.1 按频率判。节奏：同一平台批量时间隔 10 秒、每天 200 条。全部只在本机的假服务器上测过。

### `userscript`：浏览器脚本

脚本在 `userscript/`，装进使用者自己的浏览器，在「收藏」「赞」页上记下页面自己请求回来的列表里的微博链接、标题、作者昵称和发布时间，由使用者确认后交给 usuyo。不对微博发额外请求，不读 cookie。脚本带单元测试，没在真浏览器里跑过。

### `favorites`：收藏（登录进 usuyo）

清单里列出，未实现。

接口路径、访客跳转和视频地址的字段对照 [yt-dlp](https://github.com/yt-dlp/yt-dlp)（Unlicense）和 [ParseHub](https://github.com/z-mio/ParseHub)（MIT，Copyright (c) 2024 梓澪）的 `weibo.py`，回答里的字段名对照 [Weibo-archiver](https://github.com/Chilfish/Weibo-archiver)（Apache-2.0）。

## 开发

`Cargo.toml` 里的 `usuyo-plugin`、`usuyo-plugin-kit` 是对 `https://github.com/usuyo/usuyo` 的 git 依赖，钉在固定的 `rev`。

本地开发时用旁边 `usuyo/` 的工作区代替它们：`usuyo-fetch/.cargo/config.toml`（不进任何插件仓库）写了下面的覆盖，cargo 从当前目录往上找这个文件，在插件文件夹里跑 cargo 就生效；从别处跑要加 `--config <这个文件>`。

```toml
[patch."https://github.com/usuyo/usuyo"]
usuyo-plugin = { path = "../usuyo/crates/usuyo-plugin" }
usuyo-plugin-kit = { path = "../usuyo/crates/usuyo-plugin-kit" }
```

`Cargo.lock` 记的是覆盖后的路径形式，所以 CI 不加 `--locked` 构建，cargo 只重新锁这两个 usuyo crate。
