//! `ajax/statuses/show` 的回答：一条微博直接是顶层对象（yt-dlp 的 `weibo.py` 直接在回答上取 `mblogid`、`page_info`、
//! `mix_media_info`），不包一层 `data`。字段名照 Weibo-archiver 的 `packages/core/src/types/raw/statuses/mymblog.ts`
//! （Apache-2.0，只取名字和形状，没有取代码）、yt-dlp `weibo.py` 和 ParseHub `weibo.py`。
//!
//! 全部按「有就用、没有就跳过」读：回答里认不出的东西不当错误，记进 [`Post::unparsed`]，
//! 由调用方写进 `missing`，不会因此报成完整。
//!
//! 看不到的微博、要访客身份的回答、验证页的形状没有在真实的回答上量过：
//! - 访客身份不被接受：重定向去 `passport.weibo.com`（yt-dlp `_weibo_download_json` 的判断，见 `api.rs`），
//!   或回答里 `ok: -100`（`userscript/test/fixtures/error.json` 的形状，也是手写的、没量过）。
//! - 已删除：`deleted` 有值、正文是平台固定的「此微博已被删除」，或 `ok: 0` 的说明里写了不存在、已删除。
//! - 看不了：`ok: 0` 的说明里写了可见、权限一类的字。
//!
//! 认不出的 `ok != 1` 一律 `upstream-changed`，不猜成已删除。

use serde_json::Value;
use usuyo_plugin_kit::protocol::{NotFoundReason, OutputKind};

use crate::text::UrlPair;

/// 回答读不成一条微博时的几种情况。
#[derive(Debug, PartialEq)]
pub enum AnswerError {
    /// 看不到这条微博；`reason` 只在回答明确说了是删除、权限时才填。
    Unavailable {
        reason: Option<NotFoundReason>,
        message: String,
    },
    /// 站点不收这份访客身份（或没有交来）。
    VisitorRejected(String),
    /// 验证页、风控提示。
    Blocked(String),
    /// 形状不对，读不出。
    Shape(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct User {
    pub id: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pic {
    pub id: String,
    pub url: Option<String>,
    pub width: Option<u64>,
    pub height: Option<u64>,
    /// 实况图（`type: livephoto`）和动图（`gif`）的视频地址。
    pub video: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Video {
    pub url: String,
    pub duration_sec: Option<f64>,
    pub width: Option<u64>,
    pub height: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Counts {
    pub reposts: Option<u64>,
    pub comments: Option<u64>,
    pub attitudes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Retweet {
    /// 转发的原微博已删除（没有作者，正文是平台固定的一句）。
    Gone(String),
    Post(Box<Post>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Post {
    /// 十进制 `mid`；回答里没有 `mid` 时取 `idstr`、`id`。
    pub mid: String,
    pub mblogid: Option<String>,
    pub user: Option<User>,
    /// 原样的 `created_at`，例如 `Thu Oct 30 21:00:00 +0800 2025`。
    pub created_at: Option<String>,
    /// 带 HTML 的正文；没有时用 `text_raw`（已经是纯文字）。
    pub text_html: Option<String>,
    pub text_raw: Option<String>,
    pub is_long_text: bool,
    pub source_html: Option<String>,
    pub region: Option<String>,
    pub counts: Counts,
    /// 图片，按 `pic_ids` 的顺序，再接 `mix_media_info` 里没出现过的。
    pub pics: Vec<Pic>,
    /// `pic_ids` 里写了、`pic_infos` 里没有的图的个数。
    pub pics_without_info: usize,
    pub videos: Vec<Video>,
    /// 回答里有视频，但认不出地址的个数。
    pub videos_without_url: usize,
    /// 视频的封面（`page_info.page_pic`）。
    pub cover: Option<String>,
    pub urls: Vec<UrlPair>,
    pub topics: Vec<String>,
    pub retweet: Option<Retweet>,
    /// 回答里有、这里没有取的东西：属于哪一样输出，和一句原因。
    pub unparsed: Vec<(OutputKind, String)>,
}

/// 读 `show` 的回答（原始字节）。
pub fn parse_answer(bytes: &[u8]) -> Result<Post, AnswerError> {
    let value: Value = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(_) => return Err(not_json(&String::from_utf8_lossy(bytes))),
    };
    if !value.is_object() {
        return Err(AnswerError::Shape("the answer is not a JSON object".into()));
    }
    match value.get("ok").and_then(Value::as_i64) {
        Some(1) | None => {}
        Some(-100) => {
            return Err(AnswerError::VisitorRejected(
                "the answer says ok = -100 (not signed in)".into(),
            ));
        }
        Some(other) => return Err(refused(&value, other)),
    }
    if is_deleted(&value) {
        return Err(AnswerError::Unavailable {
            reason: Some(NotFoundReason::Deleted),
            message: "the post is deleted".into(),
        });
    }
    parse_post(&value).ok_or_else(|| AnswerError::Shape("the answer has no mid".into()))
}

/// 回答不是 JSON：访客页、验证页的样子，或者读不出。
fn not_json(text: &str) -> AnswerError {
    let lower = text.to_ascii_lowercase();
    if lower.contains("passport.weibo") || lower.contains("visitor") {
        AnswerError::VisitorRejected("the answer is a visitor page, not JSON".into())
    } else if lower.contains("captcha") || lower.contains("geetest") || text.contains("验证") {
        AnswerError::Blocked("the answer looks like a verification page".into())
    } else {
        AnswerError::Shape("the answer is not JSON".into())
    }
}

/// `ok` 不是 1：照说明里的字分。
fn refused(value: &Value, ok: i64) -> AnswerError {
    let msg = ["msg", "message", "error", "errmsg"]
        .iter()
        .find_map(|k| value.get(*k).and_then(Value::as_str))
        .unwrap_or_default();
    let message = format!("the answer says ok = {ok}: {msg}");
    if msg.contains("不存在") || msg.contains("已删除") || msg.contains("已被删除") {
        AnswerError::Unavailable {
            reason: Some(NotFoundReason::Deleted),
            message,
        }
    } else if msg.contains("可见") || msg.contains("权限") || msg.contains("私密") {
        AnswerError::Unavailable {
            reason: Some(NotFoundReason::Private),
            message,
        }
    } else {
        AnswerError::Shape(message)
    }
}

fn is_deleted(value: &Value) -> bool {
    let flagged = match value.get("deleted") {
        Some(Value::String(s)) => !s.is_empty() && s != "0",
        Some(Value::Number(n)) => n.as_i64().is_some_and(|n| n != 0),
        Some(Value::Bool(b)) => *b,
        _ => false,
    };
    flagged || removed_text(value.get("text_raw").or_else(|| value.get("text")))
}

/// 平台固定的一句：「此微博已被删除」「该微博已被删除」（照 userscript 里的同一条判断）。
fn removed_text(text: Option<&Value>) -> bool {
    let Some(text) = text.and_then(Value::as_str) else {
        return false;
    };
    let text = text.trim_start();
    [
        "此微博已删除",
        "此微博已被删除",
        "该微博已删除",
        "该微博已被删除",
        "抱歉，此微博已被删除",
    ]
    .iter()
    .any(|p| text.starts_with(p))
}

fn string(value: &Value, key: &str) -> Option<String> {
    match value.get(key)? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn number(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}

fn parse_post(value: &Value) -> Option<Post> {
    let mid = string(value, "mid")
        .or_else(|| string(value, "idstr"))
        .or_else(|| string(value, "id"))?;
    let user = value.get("user").filter(|u| u.is_object()).and_then(|u| {
        Some(User {
            id: string(u, "idstr").or_else(|| string(u, "id")),
            name: string(u, "screen_name")?,
        })
    });
    let mut post = Post {
        mid,
        mblogid: string(value, "mblogid"),
        user,
        created_at: string(value, "created_at"),
        text_html: string(value, "text"),
        text_raw: string(value, "text_raw"),
        is_long_text: value
            .get("isLongText")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        source_html: string(value, "source"),
        region: string(value, "region_name"),
        counts: Counts {
            reposts: number(value, "reposts_count"),
            comments: number(value, "comments_count"),
            attitudes: number(value, "attitudes_count"),
        },
        pics: Vec::new(),
        pics_without_info: 0,
        videos: Vec::new(),
        videos_without_url: 0,
        cover: None,
        urls: url_pairs(value),
        topics: topics(value),
        retweet: None,
        unparsed: Vec::new(),
    };
    pictures(value, &mut post);
    media(value, &mut post);
    post.retweet = value
        .get("retweeted_status")
        .filter(|r| r.is_object())
        .map(retweet);
    Some(post)
}

fn retweet(value: &Value) -> Retweet {
    if is_deleted(value) {
        let text = string(value, "text_raw")
            .or_else(|| string(value, "text"))
            .unwrap_or_else(|| "the original post is deleted".into());
        return Retweet::Gone(text);
    }
    match parse_post(value) {
        Some(post) => Retweet::Post(Box::new(post)),
        None => Retweet::Gone("the retweeted post has no mid".into()),
    }
}

pub fn url_pairs(value: &Value) -> Vec<UrlPair> {
    value
        .get("url_struct")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|u| {
                    Some(UrlPair {
                        short: string(u, "short_url")?,
                        long: string(u, "long_url").or_else(|| string(u, "ori_url"))?,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn topics(value: &Value) -> Vec<String> {
    value
        .get("topic_struct")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|t| string(t, "topic_title").or_else(|| string(t, "title")))
                .map(|t| t.trim_matches('#').to_owned())
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// 一张图的各种尺寸，从大到小。
const SIZES: [&str; 5] = ["largest", "original", "large", "mw2000", "bmiddle"];

fn pic_from(info: &Value, id: &str) -> Pic {
    let size = SIZES
        .iter()
        .filter_map(|k| info.get(*k))
        .find(|s| s.get("url").and_then(Value::as_str).is_some());
    Pic {
        id: id.to_owned(),
        url: size.and_then(|s| string(s, "url")),
        width: size.and_then(|s| number(s, "width")),
        height: size.and_then(|s| number(s, "height")),
        video: string(info, "video"),
    }
}

/// `pic_ids` 的顺序为准（`pic_infos` 是个以图片 id 为键的对象，不保证顺序）。
fn pictures(value: &Value, post: &mut Post) {
    let infos = value.get("pic_infos").and_then(Value::as_object);
    if let Some(ids) = value.get("pic_ids").and_then(Value::as_array) {
        for id in ids.iter().filter_map(Value::as_str) {
            match infos.and_then(|m| m.get(id)) {
                Some(info) => post.pics.push(pic_from(info, id)),
                None => post.pics_without_info += 1,
            }
        }
    }
}

/// 视频：`page_info.media_info`，再是 `mix_media_info.items[]`（图文视频混排）。
fn media(value: &Value, post: &mut Post) {
    if let Some(page) = value.get("page_info").filter(|p| p.is_object()) {
        let kind = string(page, "object_type")
            .or_else(|| string(page, "type"))
            .unwrap_or_default();
        match page.get("media_info").filter(|m| m.is_object()) {
            Some(info) => {
                add_video(post, info);
                post.cover = string(page, "page_pic");
            }
            None if kind == "video" => post.videos_without_url += 1,
            None => post.unparsed.push((
                OutputKind::Text,
                format!("page_info 的类型是 {kind:?}（文章、直播、链接卡片一类），没有取"),
            )),
        }
    }
    let Some(items) = value
        .get("mix_media_info")
        .and_then(|m| m.get("items"))
        .and_then(Value::as_array)
    else {
        return;
    };
    for item in items {
        let kind = item.get("type").and_then(Value::as_str).unwrap_or_default();
        let data = item.get("data").unwrap_or(&Value::Null);
        match kind {
            "pic" => {
                let id = string(data, "pic_id")
                    .or_else(|| string(item, "id"))
                    .unwrap_or_default();
                if !id.is_empty() && !post.pics.iter().any(|p| p.id == id) {
                    post.pics.push(pic_from(data, &id));
                }
            }
            "video" => match data.get("media_info").filter(|m| m.is_object()) {
                Some(info) => add_video(post, info),
                None => post.videos_without_url += 1,
            },
            other => post.unparsed.push((
                OutputKind::Images,
                format!("mix_media_info 里有类型 {other:?} 的一项，没有取"),
            )),
        }
    }
}

fn add_video(post: &mut Post, info: &Value) {
    match best_video(info) {
        Some(video) if !post.videos.iter().any(|v| v.url == video.url) => post.videos.push(video),
        Some(_) => {}
        None => post.videos_without_url += 1,
    }
}

/// 一路视频的地址：`playback_list` 里最清楚的 mp4（ParseHub 取第一项、yt-dlp 全列出来），
/// 没有就按 `mp4_720p_mp4`、`mp4_hd_url`、`mp4_sd_url`、`stream_url_hd`、`stream_url` 的顺序。
fn best_video(info: &Value) -> Option<Video> {
    let duration_sec = info.get("duration").and_then(Value::as_f64);
    let playback = info
        .get("playback_list")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| p.get("play_info"))
        .filter(|p| {
            p.get("mime")
                .and_then(Value::as_str)
                .is_none_or(|m| m == "video/mp4")
        })
        .filter_map(|p| Some((p, string(p, "url")?)))
        .max_by_key(|(p, _)| {
            (
                number(p, "height").unwrap_or(0),
                number(p, "bitrate").unwrap_or(0),
            )
        });
    if let Some((p, url)) = playback {
        return Some(Video {
            url,
            duration_sec,
            width: number(p, "width"),
            height: number(p, "height"),
        });
    }
    [
        "mp4_720p_mp4",
        "mp4_hd_url",
        "mp4_sd_url",
        "stream_url_hd",
        "stream_url",
    ]
    .iter()
    .find_map(|k| string(info, k))
    .map(|url| Video {
        url,
        duration_sec,
        width: None,
        height: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_that_are_not_a_post() {
        let err = |text: &str| parse_answer(text.as_bytes()).unwrap_err();
        assert!(matches!(
            err(r#"{"ok":-100,"url":"https://weibo.com/login.php"}"#),
            AnswerError::VisitorRejected(_)
        ));
        assert!(matches!(
            err(r#"{"ok":0,"msg":"该微博不存在"}"#),
            AnswerError::Unavailable {
                reason: Some(NotFoundReason::Deleted),
                ..
            }
        ));
        assert!(matches!(
            err(r#"{"ok":0,"msg":"仅粉丝可见"}"#),
            AnswerError::Unavailable {
                reason: Some(NotFoundReason::Private),
                ..
            }
        ));
        // 认不出的说明不猜成已删除
        assert!(matches!(
            err(r#"{"ok":0,"msg":"系统繁忙"}"#),
            AnswerError::Shape(_)
        ));
        assert!(matches!(
            err(r#"{"id":1,"idstr":"1","deleted":"1","text":"x"}"#),
            AnswerError::Unavailable {
                reason: Some(NotFoundReason::Deleted),
                ..
            }
        ));
        assert!(matches!(
            err("<html>passport.weibo.com/visitor</html>"),
            AnswerError::VisitorRejected(_)
        ));
        assert!(matches!(
            err("<html>captcha</html>"),
            AnswerError::Blocked(_)
        ));
        assert!(matches!(err("<html>hello</html>"), AnswerError::Shape(_)));
        assert!(matches!(err("[1]"), AnswerError::Shape(_)));
        assert!(matches!(err(r#"{"ok":1}"#), AnswerError::Shape(_)));
    }

    #[test]
    fn video_address_preference() {
        let info = serde_json::json!({
            "mp4_sd_url": "https://f.video.weibocdn.com/sd.mp4",
            "mp4_hd_url": "https://f.video.weibocdn.com/hd.mp4",
            "stream_url": "https://f.video.weibocdn.com/low.mp4",
            "duration": 12.5,
            "playback_list": [
                {"play_info": {"url": "https://f.video.weibocdn.com/a.mp4", "mime": "video/mp4", "height": 480, "width": 270, "bitrate": 900}},
                {"play_info": {"url": "https://f.video.weibocdn.com/b.mp4", "mime": "video/mp4", "height": 720, "width": 406, "bitrate": 1500}},
                {"play_info": {"url": "https://f.video.weibocdn.com/c.jpg", "mime": "image/jpeg", "height": 1080}}
            ]
        });
        let video = best_video(&info).unwrap();
        assert_eq!(video.url, "https://f.video.weibocdn.com/b.mp4");
        assert_eq!(video.height, Some(720));
        assert_eq!(video.duration_sec, Some(12.5));
        let plain = serde_json::json!({"mp4_sd_url": "s", "mp4_hd_url": "h"});
        assert_eq!(best_video(&plain).unwrap().url, "h");
        assert!(best_video(&serde_json::json!({})).is_none());
    }
}
