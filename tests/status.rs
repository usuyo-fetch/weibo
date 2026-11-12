//! 对着本机的假服务器跑方法 `status`：文字、图片、视频、转发、长文、各种错误回答、输出目录里有什么、
//! 谁带了访客身份。不碰真网络。
//!
//! `fixtures/` 里的回答都是手写的，内容是编的，每个文件的 `_note` 写了没有量过、形状照哪里写。
//! 看不到的微博、访客身份被拒、验证页的回答（下面几个内联的）同样没有在真实的回答上量过，
//! 见 `src/post.rs` 顶部。

mod support;

use std::net::IpAddr;
use std::path::Path;

use support::{Request, Response, Server, fixture};
use usuyo_fetch_weibo::api::{Endpoints, Weibo};
use usuyo_fetch_weibo::status;
use usuyo_plugin::protocol::{
    CallFailure, Completeness, ErrorCode, FetchOutput, ItemKind, NotFoundReason, OutputKind,
};
use usuyo_plugin_kit::http::{Cookie, Policy};

const TEXT: &str = "4900000000000001";
const IMAGES: &str = "4900000000000002";
const VIDEO: &str = "4900000000000003";
const RETWEET: &str = "4900000000000004";
const GONE: &str = "4900000000000005";
const LONG: &str = "4900000000000006";
const MIX: &str = "4900000000000007";

/// 只在测试里用的 id：各自触发一种服务器行为。
const DELETED: &str = "4900000000000011";
const PRIVATE: &str = "4900000000000012";
const UNKNOWN_REFUSAL: &str = "4900000000000013";
const NOT_JSON: &str = "4900000000000014";
const HTTP_403: &str = "4900000000000015";
const HTTP_429: &str = "4900000000000016";
const HTTP_500: &str = "4900000000000017";
const REDIRECT_ELSEWHERE: &str = "4900000000000018";
const WRONG_POST: &str = "4900000000000019";
const LONG_FAILS: &str = "4900000000000020";
const BAD_IMAGE: &str = "4900000000000021";
const EVIL_IMAGE: &str = "4900000000000022";
const BAD_VIDEO: &str = "4900000000000023";
const MIX_UNKNOWN: &str = "4900000000000024";
const VISITOR_PAGE: &str = "4900000000000025";
const LOGGED_OUT: &str = "4900000000000026";
const ARTICLE: &str = "4900000000000027";
const LONG_NO_CONTENT: &str = "4900000000000028";

const GOOD_COOKIE: &str = "SUB=visitor-sub-1";

fn local_policy() -> Policy {
    Policy {
        lookup: |_, _| {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no DNS in tests",
            ))
        },
        public: |ip| ip == IpAddr::from([127, 0, 0, 1]),
        cookies_over_http: true,
        ..Policy::strict()
    }
}

fn json(text: String) -> Response {
    Response {
        status: 200,
        headers: vec![("Content-Type", "application/json; charset=utf-8".into())],
        body: text.into_bytes(),
    }
}

/// 一份换了 mid 的回答。
fn as_post(name: &str, from: &str, to: &str, base: &str) -> String {
    fixture(name, base).replace(from, to)
}

fn show(id: &str, base: &str) -> Response {
    match id {
        TEXT => json(fixture("show_text.json", base)),
        IMAGES => json(fixture("show_images.json", base)),
        VIDEO => json(fixture("show_video.json", base)),
        RETWEET => json(fixture("show_retweet.json", base)),
        GONE => json(fixture("show_retweet_gone.json", base)),
        LONG => json(fixture("show_long.json", base)),
        MIX => json(fixture("show_mix.json", base)),
        DELETED => json(r#"{"ok":0,"msg":"该微博不存在"}"#.into()),
        PRIVATE => json(r#"{"ok":0,"msg":"博主设置了仅粉丝可见"}"#.into()),
        UNKNOWN_REFUSAL => json(r#"{"ok":0,"msg":"系统繁忙，请稍后再试"}"#.into()),
        NOT_JSON => Response {
            status: 200,
            headers: vec![("Content-Type", "text/html".into())],
            body: b"<html><body>hello</body></html>".to_vec(),
        },
        VISITOR_PAGE => Response {
            status: 200,
            headers: vec![("Content-Type", "text/html".into())],
            body: b"<html><script>location='https://passport.weibo.com/visitor/visitor?a=enter'</script></html>".to_vec(),
        },
        LOGGED_OUT => json(r#"{"ok":-100,"url":"https://weibo.com/login.php"}"#.into()),
        HTTP_403 => Response::status(403),
        HTTP_429 => Response::status(429).header("Retry-After", "30"),
        HTTP_500 => Response::status(500),
        REDIRECT_ELSEWHERE => Response::redirect("https://weibo.com/somewhere"),
        // 答的是另一条微博
        WRONG_POST => json(fixture("show_text.json", base)),
        LONG_FAILS => json(as_post("show_long.json", "4900000000000006", LONG_FAILS, base).replace("N00000006", "FAILLONG1")),
        LONG_NO_CONTENT => json(as_post("show_long.json", "4900000000000006", LONG_NO_CONTENT, base).replace("N00000006", "NOCONTENT")),
        BAD_IMAGE => json(as_post("show_images.json", "4900000000000002", BAD_IMAGE, base).replace("/img/b.png", "/missing/b.png")),
        EVIL_IMAGE => json(as_post("show_images.json", "4900000000000002", EVIL_IMAGE, base).replace(&format!("{base}/img/a.jpg"), "https://evil.example/a.jpg")),
        BAD_VIDEO => json(as_post("show_video.json", "4900000000000003", BAD_VIDEO, base).replace("/vid/p720.mp4", "/missing/p720.mp4")),
        MIX_UNKNOWN => json(
            as_post("show_mix.json", "4900000000000007", MIX_UNKNOWN, base)
                .replace(
                    r#"{"id": "vid1", "type": "video""#,
                    r#"{"id": "x", "type": "article", "data": {}}, {"id": "vid1", "type": "video""#,
                ),
        ),
        ARTICLE => json(
            r#"{"ok":1,"mid":"4900000000000027","mblogid":"ARTICLE01","text":"文章","text_raw":"文章","user":{"id":1,"idstr":"1","screen_name":"某"},"page_info":{"type":"article","object_type":"article","page_title":"标题"}}"#.into(),
        ),
        _ => Response::status(404),
    }
}

fn longtext(id: &str, base: &str) -> Response {
    match id {
        "N00000006" => json(fixture("longtext.json", base)),
        "NOCONTENT" => json(r#"{"ok":1,"data":{}}"#.into()),
        _ => Response::status(500),
    }
}

/// 一个什么都有的微博：接口要访客 cookie，图片视频不看 cookie。
fn weibo(req: &Request, base: &str) -> Response {
    match req.path.as_str() {
        "/ajax/statuses/show" | "/ajax/statuses/longtext" => {
            if req.cookie() != Some(GOOD_COOKIE) {
                return Response::redirect(
                    "https://passport.weibo.com/visitor/visitor?entry=miniblog",
                );
            }
            let id = req.query.get("id").map(String::as_str).unwrap_or_default();
            if req.path.ends_with("longtext") {
                longtext(id, base)
            } else {
                show(id, base)
            }
        }
        // 文件内容就是请求的路径，测试靠它认出每个文件是哪个地址下的
        p if p.starts_with("/img/") || p.starts_with("/vid/") => Response::bytes(p.as_bytes()),
        _ => Response::status(404),
    }
}

struct Outcome {
    server: Server,
    out: tempfile::TempDir,
    result: Result<FetchOutput, CallFailure>,
}

fn run_with(
    id: &str,
    cookie: Option<&str>,
    want: Option<&[OutputKind]>,
    link: Option<String>,
) -> Outcome {
    let server = Server::start(weibo);
    let api = Weibo::new(Endpoints::local(&server.base, local_policy()));
    let cookie = cookie.map(|c| Cookie::new(c, ["127.0.0.1"]));
    let out = tempfile::tempdir().unwrap();
    let link = link.unwrap_or_else(|| format!("https://weibo.com/1700000001/{id}"));
    let result = status::fetch(&api, cookie.as_ref(), &link, out.path(), want);
    Outcome {
        server,
        out,
        result,
    }
}

fn run(id: &str) -> Outcome {
    run_with(id, Some(GOOD_COOKIE), None, None)
}

fn ok(o: &Outcome) -> &FetchOutput {
    o.result.as_ref().unwrap_or_else(|e| panic!("{e:?}"))
}

fn err(o: &Outcome) -> &CallFailure {
    o.result.as_ref().unwrap_err()
}

fn read(o: &Outcome, name: &str) -> Vec<u8> {
    std::fs::read(o.out.path().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn files(dir: &Path) -> Vec<String> {
    let mut all = Vec::new();
    fn walk(dir: &Path, base: &Path, all: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, base, all);
            } else {
                all.push(
                    path.strip_prefix(base)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    walk(dir, dir, &mut all);
    all.sort();
    all
}

fn detail(f: &CallFailure) -> &str {
    f.detail.as_deref().map_or("", String::as_str)
}

fn paths(o: &Outcome) -> Vec<String> {
    o.server.paths()
}

#[test]
fn a_text_post() {
    let o = run(TEXT);
    let out = ok(&o);
    assert_eq!(out.completeness, Completeness::Full);
    assert!(out.missing.is_empty() && out.missing_reason.is_none());
    assert_eq!(out.kind, ItemKind::Post);
    assert_eq!(
        out.text.as_deref(),
        Some(
            "示例微博 & 第一行\n第二行 @某人 #示例话题# https://example.invalid/long/page?a=1&b=2"
        )
    );
    assert_eq!(serde_json::to_value(out).unwrap()["textFormat"], "plain");
    assert_eq!(out.title.as_deref(), Some("示例微博 & 第一行"));
    let author = out.author.as_ref().unwrap();
    assert_eq!(author.name, "示例博主甲");
    assert_eq!(author.platform_id.as_deref(), Some("1700000001"));
    assert_eq!(
        author.url.as_deref(),
        Some("https://weibo.com/u/1700000001")
    );
    assert_eq!(
        out.published_at.as_ref().unwrap().as_str(),
        "2025-10-30T13:00:00Z"
    );
    assert_eq!(out.platform_tags, ["示例话题"]);
    assert!(out.images.is_empty() && out.media.is_empty());

    // 原始回答一个字节都不改
    assert_eq!(
        read(&o, "status.json"),
        fixture("show_text.json", &o.server.base).into_bytes()
    );
    let meta: serde_json::Value = serde_json::from_slice(&read(&o, "meta.json")).unwrap();
    assert_eq!(meta["mid"], TEXT);
    assert_eq!(meta["mblogid"], "N00000001");
    assert_eq!(meta["url"], format!("https://m.weibo.cn/detail/{TEXT}"));
    assert_eq!(
        meta["counts"],
        serde_json::json!({"reposts": 2, "comments": 3, "attitudes": 5})
    );
    assert_eq!(meta["source"], "示例客户端");
    assert_eq!(meta["regionName"], "发布于 示例");
    assert_eq!(files(o.out.path()), ["meta.json", "status.json"]);

    // 一个请求：带访客 cookie、Referer，id 是链接里的那一段
    let log = o.server.requests();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].path, "/ajax/statuses/show");
    assert_eq!(log[0].query["id"], TEXT);
    assert_eq!(log[0].query["isGetLongText"], "true");
    assert_eq!(log[0].cookie(), Some(GOOD_COOKIE));
    assert_eq!(log[0].headers["referer"], "https://weibo.com/");
}

#[test]
fn links_with_a_mblogid_ask_for_the_same_post() {
    // 链接里是 mblogid：换成 mid 再问，身份是 mid
    let o = run_with(
        TEXT,
        Some(GOOD_COOKIE),
        None,
        Some("https://weibo.com/detail/N00000001".into()),
    );
    assert_eq!(o.server.requests()[0].query["id"], TEXT);
    assert_eq!(ok(&o).completeness, Completeness::Full);
}

#[test]
fn pictures_follow_pic_ids_and_live_photos_bring_their_video() {
    let o = run(IMAGES);
    let out = ok(&o);
    assert_eq!(out.completeness, Completeness::Full);
    // pic_ids 的顺序：c、a、b（不是 pic_infos 的键序）；尺寸取 largest，再 original
    let names: Vec<&str> = out.images.iter().map(|p| p.as_str()).collect();
    assert_eq!(
        names,
        ["images/001.jpg", "images/002.jpg", "images/003.png"]
    );
    assert_eq!(read(&o, "images/001.jpg"), b"/img/c.jpg");
    assert_eq!(read(&o, "images/002.jpg"), b"/img/a.jpg");
    assert_eq!(read(&o, "images/003.png"), b"/img/b.png");
    assert_eq!(out.media.len(), 1);
    assert_eq!(out.media[0].path.as_str(), "media/live-001.mp4");
    assert_eq!(read(&o, "media/live-001.mp4"), b"/vid/c-live.mp4");
    assert_eq!(out.kind, ItemKind::Post);
    // 访客 cookie 只给接口，图片和视频的请求不带
    for r in o.server.requests() {
        if r.path.starts_with("/img/") || r.path.starts_with("/vid/") {
            assert_eq!(r.cookie(), None, "{}", r.path);
            assert_eq!(r.headers["referer"], "https://weibo.com/");
        } else {
            assert_eq!(r.cookie(), Some(GOOD_COOKIE));
        }
    }
}

#[test]
fn a_video_post_takes_the_clearest_mp4_and_the_cover() {
    let o = run(VIDEO);
    let out = ok(&o);
    assert_eq!(out.completeness, Completeness::Full);
    assert_eq!(out.kind, ItemKind::Video);
    assert_eq!(out.media.len(), 1);
    assert_eq!(out.media[0].path.as_str(), "media/video-001.mp4");
    assert_eq!(out.media[0].duration_sec, Some(12.5));
    assert_eq!(read(&o, "media/video-001.mp4"), b"/vid/p720.mp4");
    assert_eq!(out.images.len(), 1);
    assert_eq!(out.images[0].as_str(), "images/cover.jpg");
    // 只下了一路视频
    assert_eq!(
        paths(&o).iter().filter(|p| p.starts_with("/vid/")).count(),
        1
    );
}

#[test]
fn a_retweet_is_a_quoted_block_with_its_own_media() {
    let o = run(RETWEET);
    let out = ok(&o);
    assert_eq!(out.completeness, Completeness::Full);
    assert_eq!(
        out.text.as_deref(),
        Some(
            "转发理由\n第二行\n\n> 转发 @示例原作者 https://weibo.com/1700000104/N0000001G\n> 原微博的字\n> 带 https://example.invalid/original"
        )
    );
    assert_eq!(serde_json::to_value(out).unwrap()["textFormat"], "plain");
    // 标题和作者是本条的，不是转发的那一条的
    assert_eq!(out.title.as_deref(), Some("转发理由"));
    assert_eq!(out.author.as_ref().unwrap().name, "示例博主丁");
    assert_eq!(out.images.len(), 1);
    assert_eq!(out.images[0].as_str(), "images/q-001.jpg");
    assert_eq!(read(&o, "images/q-001.jpg"), b"/img/q.jpg");
    let meta: serde_json::Value = serde_json::from_slice(&read(&o, "meta.json")).unwrap();
    assert_eq!(meta["quoted"]["mid"], "4900000000000104");
    assert_eq!(meta["quoted"]["author"]["screenName"], "示例原作者");
    assert_eq!(meta["quoted"]["counts"]["attitudes"], 9);
}

#[test]
fn a_retweet_of_a_deleted_post_still_succeeds() {
    let o = run(GONE);
    let out = ok(&o);
    assert_eq!(out.completeness, Completeness::Full);
    assert_eq!(
        out.text.as_deref(),
        Some(
            "转发微博\n\n> 转发的原微博看不到了：此微博已被删除。查看帮助：http://t.cn/A6exampleB"
        )
    );
    let meta: serde_json::Value = serde_json::from_slice(&read(&o, "meta.json")).unwrap();
    assert!(
        meta["quoted"]["gone"]
            .as_str()
            .unwrap()
            .contains("已被删除")
    );
}

#[test]
fn a_long_post_takes_the_long_text() {
    let o = run(LONG);
    let out = ok(&o);
    assert_eq!(out.completeness, Completeness::Full);
    assert_eq!(
        out.text.as_deref(),
        Some("长文的前一段，后一段\n第二段 https://example.invalid/from-longtext")
    );
    let log = o.server.requests();
    assert_eq!(log[1].path, "/ajax/statuses/longtext");
    assert_eq!(log[1].query["id"], "N00000006");
    assert_eq!(log[1].cookie(), Some(GOOD_COOKIE));
    assert_eq!(
        read(&o, "longtext.json"),
        fixture("longtext.json", &o.server.base).into_bytes()
    );
}

#[test]
fn a_long_text_that_cannot_be_had_is_partial_not_full() {
    for id in [LONG_FAILS, LONG_NO_CONTENT] {
        let o = run(id);
        let out = ok(&o);
        assert_eq!(out.completeness, Completeness::Partial, "{id}");
        assert_eq!(out.missing, [OutputKind::Text], "{id}");
        assert!(
            out.missing_reason.as_deref().unwrap().contains("长文"),
            "{id}"
        );
        // 留着 show 给的前一段
        assert_eq!(out.text.as_deref(), Some("长文的前一段…全文"), "{id}");
    }
}

#[test]
fn mixed_media_gives_pictures_and_videos() {
    let o = run(MIX);
    let out = ok(&o);
    assert_eq!(out.completeness, Completeness::Full);
    assert_eq!(out.images.len(), 1);
    assert_eq!(out.media.len(), 1);
    assert_eq!(read(&o, "media/video-001.mp4"), b"/vid/mix.mp4");
    // 有图又有视频的算图文
    assert_eq!(out.kind, ItemKind::Post);
}

#[test]
fn parts_the_plugin_does_not_read_make_the_answer_partial() {
    let o = run(MIX_UNKNOWN);
    let out = ok(&o);
    assert_eq!(out.completeness, Completeness::Partial);
    assert_eq!(out.missing, [OutputKind::Images]);
    assert!(out.missing_reason.as_deref().unwrap().contains("article"));
    assert_eq!(out.images.len(), 1);

    let o = run(ARTICLE);
    let out = ok(&o);
    assert_eq!(out.completeness, Completeness::Partial);
    assert_eq!(out.missing, [OutputKind::Text]);
}

#[test]
fn a_failed_download_is_recorded_and_the_rest_is_kept() {
    let o = run(BAD_IMAGE);
    let out = ok(&o);
    assert_eq!(out.completeness, Completeness::Partial);
    assert_eq!(out.missing, [OutputKind::Images]);
    assert_eq!(out.images.len(), 2);
    assert!(out.missing_reason.is_some());
    // 下不来的那张不留文件
    assert_eq!(
        files(o.out.path())
            .iter()
            .filter(|f| f.starts_with("images/"))
            .count(),
        2
    );

    let o = run(EVIL_IMAGE);
    let out = ok(&o);
    assert_eq!(out.completeness, Completeness::Partial);
    assert!(
        out.missing_reason
            .as_deref()
            .unwrap()
            .contains("evil.example")
    );
    // 名单外的主机一次也没连
    assert!(paths(&o).iter().all(|p| !p.contains("evil")));

    let o = run(BAD_VIDEO);
    let out = ok(&o);
    assert_eq!(out.completeness, Completeness::Partial);
    assert_eq!(out.missing, [OutputKind::Video]);
    assert!(out.media.is_empty());
    assert_eq!(out.images.len(), 1);
}

#[test]
fn only_what_is_wanted_is_fetched() {
    let o = run_with(IMAGES, Some(GOOD_COOKIE), Some(&[OutputKind::Text]), None);
    let out = ok(&o);
    assert_eq!(out.completeness, Completeness::Full);
    assert_eq!(out.text.as_deref(), Some("三张图"));
    assert!(
        out.images.is_empty()
            && out.media.is_empty()
            && out.author.is_none()
            && out.title.is_none()
    );
    assert_eq!(paths(&o), ["/ajax/statuses/show"]);
    assert_eq!(files(o.out.path()), ["status.json"]);

    let o = run_with(VIDEO, Some(GOOD_COOKIE), Some(&[OutputKind::Video]), None);
    let out = ok(&o);
    assert_eq!(out.completeness, Completeness::Full);
    assert_eq!(out.media.len(), 1);
    assert!(out.images.is_empty() && out.text.is_none());
}

#[test]
fn without_a_visitor_identity_nothing_is_requested() {
    let o = run_with(TEXT, None, None, None);
    let f = err(&o);
    assert_eq!(f.code, ErrorCode::VisitorRequired);
    assert!(f.message.contains("no visitor identity"));
    assert!(o.server.requests().is_empty());
    assert!(files(o.out.path()).is_empty());
}

#[test]
fn a_link_that_is_not_a_post_is_turned_away_before_anything_else() {
    for link in [
        "https://weibo.com/u/1700000001",
        "https://example.com/1700000001/N00000001",
    ] {
        for cookie in [Some(GOOD_COOKIE), None] {
            let o = run_with(TEXT, cookie, None, Some(link.into()));
            assert_eq!(err(&o).code, ErrorCode::UnsupportedInput, "{link}");
            assert!(o.server.requests().is_empty());
        }
    }
    let o = run_with(TEXT, Some(GOOD_COOKIE), Some(&[OutputKind::Html]), None);
    assert_eq!(err(&o).code, ErrorCode::InvalidArgs);
    assert!(o.server.requests().is_empty());
}

#[test]
fn a_visitor_identity_the_site_does_not_accept() {
    // 接口把请求送去 passport.weibo.com：不跟，Location 进 detail
    let o = run_with(TEXT, Some("SUB=expired"), None, None);
    let f = err(&o);
    assert_eq!(f.code, ErrorCode::VisitorRequired);
    assert!(detail(f).contains("passport.weibo.com/visitor"), "{f:?}");
    assert_eq!(o.server.requests().len(), 1);
    assert!(files(o.out.path()).is_empty());

    for id in [LOGGED_OUT, VISITOR_PAGE] {
        let o = run(id);
        let f = err(&o);
        assert_eq!(f.code, ErrorCode::VisitorRequired, "{id}");
        assert!(
            !detail(f).is_empty(),
            "{id}: a rejection from an answer carries the answer"
        );
    }
    assert!(detail(err(&run(LOGGED_OUT))).contains("login.php"));
}

#[test]
fn answers_about_a_post_that_cannot_be_seen() {
    let o = run(DELETED);
    let f = err(&o);
    assert_eq!(
        (f.code, f.reason),
        (ErrorCode::NotFound, Some(NotFoundReason::Deleted))
    );
    assert!(detail(f).contains("不存在"));

    let o = run(PRIVATE);
    assert_eq!(
        (err(&o).code, err(&o).reason),
        (ErrorCode::NotFound, Some(NotFoundReason::Private))
    );

    // 认不出的回答不猜成已删除
    let o = run(UNKNOWN_REFUSAL);
    let f = err(&o);
    assert_eq!((f.code, f.reason), (ErrorCode::UpstreamChanged, None));
    assert!(detail(f).contains("系统繁忙"));

    let o = run(NOT_JSON);
    let f = err(&o);
    assert_eq!(f.code, ErrorCode::UpstreamChanged);
    assert!(detail(f).contains("hello"));

    let o = run(WRONG_POST);
    let f = err(&o);
    assert_eq!(f.code, ErrorCode::UpstreamChanged);
    assert!(f.message.contains(TEXT) && !detail(f).is_empty());
    assert!(files(o.out.path()).is_empty());

    // 没有这条微博的 id：接口回 404，不说是不是删了
    let o = run("4900000000000099");
    assert_eq!((err(&o).code, err(&o).reason), (ErrorCode::NotFound, None));
}

#[test]
fn http_statuses_map_to_error_codes() {
    let o = run(HTTP_403);
    assert_eq!(err(&o).code, ErrorCode::Blocked);
    let o = run(HTTP_429);
    assert_eq!(
        (err(&o).code, err(&o).retry_after),
        (ErrorCode::RateLimited, Some(30))
    );
    let o = run(HTTP_500);
    assert_eq!(err(&o).code, ErrorCode::Network);
    let o = run(REDIRECT_ELSEWHERE);
    let f = err(&o);
    assert_eq!(f.code, ErrorCode::UpstreamChanged);
    assert!(detail(f).contains("https://weibo.com/somewhere"));
    // 跳转目标一次也没请求
    assert_eq!(o.server.requests().len(), 1);
}
