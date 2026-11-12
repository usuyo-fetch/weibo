# usuyo-fetch/weibo

微博的获取插件，供 [usuyo](https://github.com/usuyo/usuyo) 调用：按链接取回单条微博。

| 方法 | 清单里的 id | 取得 | 账号 | 层级 | 现状 |
|---|---|---|---|---|---|
| 公开微博页的数据接口 | `status` | 单条微博：文字（长文）、作者、发布时间、计数、来源、图片、视频、转发的那一条、原始回答 | 访客身份（在 usuyo 里取一次，不是账号） | `public-web` | 已实现，只在本机的假服务器上测过 |
| 浏览器脚本 | `userscript` | 存过的：收藏、赞过（脚本在 `userscript/`，把链接交给 usuyo） | 只在使用者自己的浏览器里 | `browser-session` | 脚本已做，没在真浏览器里跑过 |
| 收藏 | `favorites` | 存过的：收藏，能跟着收 | 登录进 usuyo | `session-api` | 未实现 |
| MediaCrawler 的结果文件 | `mediacrawler` | 存下的：你自己跑 MediaCrawler 留下的微博帖子文件（json 或 jsonl，或把 data 文件夹压成的 zip）里每篇的链接、标题、作者昵称、发布时间；不读评论、CSV、SQLite；只列链接，内容交给本插件的其他方法去取 | 不使用（文件是使用者自己用登录了账号的 MediaCrawler 跑出来的，这种方法不联网） | `private-api`，默认关 | 已实现，只对着手写的文件测过，没有对着 MediaCrawler 真正的输出比对过 |

接口路径、访客跳转和视频地址的字段对照 [yt-dlp](https://github.com/yt-dlp/yt-dlp)（Unlicense）和 [ParseHub](https://github.com/z-mio/ParseHub)（MIT，Copyright (c) 2024 梓澪）的 `weibo.py`，回答里的字段名对照 [Weibo-archiver](https://github.com/Chilfish/Weibo-archiver)（Apache-2.0）。
