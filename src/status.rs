//! 方法 `status`：一条链接取一条微博（协议 §3.6、§3.7）。要一份宿主交来的访客身份；没有就不发请求，直接回 `visitor-required`。
//!
//! 输出目录里的文件名都是固定的，不用网络上来的名字：
//! `status.json`（`show` 回答的原始字节）、`longtext.json`、`q-longtext.json`（长文的原始字节）、`meta.json`、
//! `images/001.<ext>`…、`images/cover.<ext>`（视频的封面）、`media/video-001.mp4`…、`media/live-001.mp4`（实况图的视频）。
//! 转发的那一条的文件名前面加 `q-`（和 X 插件的引用帖子一样）。
//!
//! 转发的微博放进 `text` 的一段引用（每行前面 `> `，`textFormat` 仍是 `plain`），转发的那条的图片和视频也一起下，
//! 它的作者、时间、计数在 `meta.json` 的 `quoted` 里。

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use usuyo_plugin_kit::http::Cookie;
use usuyo_plugin_kit::protocol::{
    Access, Author, CallFailure, Completeness, ErrorCode, Evidence, FetchOutput, ItemKind, Media,
    MediaKind, OutputKind, RelPath, Rfc3339, TextFormat,
};

use crate::api::{ApiError, Weibo, blocked, not_found, step, visitor_missing, visitor_rejected};
use crate::canonical::identity;
use crate::ids::{Parsed, mblogid_from_mid, parse_input};
use crate::post::{AnswerError, Pic, Post, Retweet, parse_answer, url_pairs};
use crate::text::html_to_text;

/// 本方法能给的输出，和清单里的 `outputs` 一致。
pub const OUTPUTS: [OutputKind; 4] = [
    OutputKind::Text,
    OutputKind::Images,
    OutputKind::Video,
    OutputKind::Meta,
];

const SHAPE_HINT: &str = "微博的接口回答变了，这种方法暂时取不到；可以换别的方法";
const IMAGE_BYTES: u64 = 40 << 20;
const IMAGE_TIMEOUT: Duration = Duration::from_secs(30);
const VIDEO_BYTES: u64 = 2 << 30;
const VIDEO_TIMEOUT: Duration = Duration::from_secs(20 * 60);
/// 一条微博最多 18 张图（实际上限更小），本条和转发的加起来最多存这么多。
const MAX_IMAGES: usize = 100;
const TITLE_CHARS: usize = 100;

/// 方法 `status`。`cookie` 是宿主交来的访客身份，没有就是 `None`。
pub fn fetch(
    api: &Weibo,
    cookie: Option<&Cookie>,
    input: &str,
    out: &Path,
    want: Option<&[OutputKind]>,
) -> Result<FetchOutput, CallFailure> {
    let wanted: BTreeSet<OutputKind> =
        want.map_or_else(|| OUTPUTS.into(), |w| w.iter().copied().collect());
    if let Some(other) = wanted.iter().find(|w| !OUTPUTS.contains(w)) {
        return Err(CallFailure::new(
            ErrorCode::InvalidArgs,
            format!("cannot give `{other}`"),
        ));
    }
    let Parsed::Post { mid } = parse_input(input) else {
        return Err(CallFailure::new(
            ErrorCode::UnsupportedInput,
            "not a weibo post link",
        ));
    };
    let identity = identity(&mid)?;
    // 没有访客身份就不去问：公开接口不带它只会被送去访客页
    let cookie = cookie.ok_or_else(visitor_missing)?;

    let fetched_at = Rfc3339::from_timestamp(jiff::Timestamp::now());
    let raw = api.show(&mid, cookie)?;
    let post = parse_answer(&raw).map_err(|e| answer_failure(e, &raw))?;
    if post.mid != mid {
        return Err(CallFailure {
            hint: Some(SHAPE_HINT.into()),
            ..CallFailure::new(
                ErrorCode::UpstreamChanged,
                format!("the answer is for post {} instead of {mid}", post.mid),
            )
        }
        .with_detail(&String::from_utf8_lossy(&raw)));
    }
    write(out, "status.json", &raw)?;

    let mut run = Run {
        api,
        out,
        wanted: &wanted,
        partial: BTreeSet::new(),
        absent: BTreeSet::new(),
        reasons: Vec::new(),
        failed: Vec::new(),
        images: Vec::new(),
        media: Vec::new(),
        image_records: Vec::new(),
        video_records: Vec::new(),
    };

    let mut post = post;
    if wanted.contains(&OutputKind::Text) {
        run.long_text(cookie, &mut post, "longtext.json", "")?;
        if let Some(Retweet::Post(q)) = post.retweet.as_mut() {
            run.long_text(cookie, q, "q-longtext.json", "转发的")?;
        }
    }
    for (kind, why) in post.unparsed.iter().chain(
        quoted(&post)
            .map(|q| q.unparsed.iter())
            .into_iter()
            .flatten(),
    ) {
        run.partial.insert(*kind);
        run.reasons.push(why.clone());
    }

    let own_text = text_of(&post);
    let text = if wanted.contains(&OutputKind::Text) {
        compose_text(&own_text, post.retweet.as_ref())
    } else {
        None
    };
    if wanted.contains(&OutputKind::Text) && text.is_none() {
        run.absent.insert(OutputKind::Text);
    }

    run.download_media(&post, "")?;
    if let Some(q) = quoted(&post) {
        run.download_media(q, "q-")?;
    }
    if post.pics.is_empty()
        && post.cover.is_none()
        && quoted(&post).is_none_or(|q| q.pics.is_empty() && q.cover.is_none())
        && post.pics_without_info == 0
    {
        run.absent.insert(OutputKind::Images);
    }
    let has_live = |p: &Post| p.pics.iter().any(|p| p.video.is_some());
    if post.videos.is_empty()
        && post.videos_without_url == 0
        && !has_live(&post)
        && quoted(&post)
            .is_none_or(|q| q.videos.is_empty() && q.videos_without_url == 0 && !has_live(q))
    {
        run.absent.insert(OutputKind::Video);
    }

    let meta_wanted = wanted.contains(&OutputKind::Meta);
    let published_at = post
        .created_at
        .as_deref()
        .and_then(|c| match parse_time(c) {
            Ok(t) => Some(t),
            Err(why) => {
                run.reasons.push(format!("发布时间：{why}"));
                None
            }
        });
    let author = post.user.as_ref().map(|u| Author {
        name: u.name.clone(),
        platform_id: u.id.clone(),
        url: u.id.as_ref().map(|id| format!("https://weibo.com/u/{id}")),
    });
    let title = first_line(&own_text);
    if meta_wanted {
        if title.is_none() && author.is_none() && published_at.is_none() && post.topics.is_empty() {
            run.absent.insert(OutputKind::Meta);
        }
        write(
            out,
            "meta.json",
            &serde_json::to_vec_pretty(&run.meta(&post))
                .map_err(|e| CallFailure::new(ErrorCode::Internal, format!("meta.json: {e}")))?,
        )?;
    }

    let mut output = FetchOutput {
        identity: Some(identity),
        kind: if post.videos.is_empty() || !post.pics.is_empty() {
            ItemKind::Post
        } else {
            ItemKind::Video
        },
        title: title.filter(|_| meta_wanted),
        author: author.filter(|_| meta_wanted),
        published_at: published_at.filter(|_| meta_wanted),
        text_format: text.as_ref().map(|_| TextFormat::Plain),
        text,
        html: None,
        images: std::mem::take(&mut run.images),
        media: std::mem::take(&mut run.media),
        platform_tags: if meta_wanted {
            post.topics.clone()
        } else {
            Vec::new()
        },
        comments: None,
        completeness: Completeness::Partial,
        missing: Vec::new(),
        missing_reason: None,
        evidence: Evidence {
            fetched_at,
            access: Access::PublicWeb,
            site: None,
            upstream: None,
            status: Some(200),
        },
        credentials_update: None,
    };
    let obtained = output.obtained();
    output.missing = wanted
        .iter()
        .filter(|kind| {
            !run.absent.contains(kind) && (run.partial.contains(kind) || !obtained.contains(kind))
        })
        .copied()
        .collect();
    output.completeness = if output.missing.is_empty() {
        Completeness::Full
    } else {
        Completeness::Partial
    };
    if !run.reasons.is_empty() {
        output.missing_reason = Some(run.reasons.join("；"));
    }
    Ok(output)
}

/// 回答读不成一条微博时的错误码；回答的开头一律放进 `detail`。
fn answer_failure(err: AnswerError, raw: &[u8]) -> CallFailure {
    let head = String::from_utf8_lossy(raw);
    match err {
        AnswerError::Unavailable { reason, message } => not_found(reason, message),
        AnswerError::VisitorRejected(message) => visitor_rejected(message),
        AnswerError::Blocked(message) => blocked(message),
        AnswerError::Shape(message) => CallFailure {
            hint: Some(SHAPE_HINT.into()),
            ..CallFailure::new(ErrorCode::UpstreamChanged, message)
        },
    }
    .with_detail(&head)
}

fn quoted(post: &Post) -> Option<&Post> {
    match &post.retweet {
        Some(Retweet::Post(q)) => Some(q),
        _ => None,
    }
}

/// 本条微博的纯文字。
fn text_of(post: &Post) -> String {
    match (&post.text_html, &post.text_raw) {
        (Some(html), _) => html_to_text(html, &post.urls),
        (None, Some(raw)) => raw.trim().to_owned(),
        (None, None) => String::new(),
    }
}

/// 本条的文字，再接转发的那一条（每行前面 `> `）。
fn compose_text(own: &str, retweet: Option<&Retweet>) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if !own.is_empty() {
        parts.push(own.to_owned());
    }
    match retweet {
        Some(Retweet::Post(q)) => {
            let who = match (&q.user, &q.mblogid) {
                (Some(u), Some(b)) => format!(
                    "@{} https://weibo.com/{}/{b}",
                    u.name,
                    u.id.as_deref().unwrap_or("0")
                ),
                (Some(u), None) => format!("@{}", u.name),
                (None, _) => crate::ids::canonical_url(&q.mid),
            };
            let mut block = format!("> 转发 {who}");
            for line in text_of(q).lines() {
                block.push_str("\n>");
                if !line.is_empty() {
                    block.push(' ');
                    block.push_str(line);
                }
            }
            parts.push(block);
        }
        Some(Retweet::Gone(why)) => parts.push(format!("> 转发的原微博看不到了：{}", why.trim())),
        None => {}
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

fn first_line(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(line.chars().take(TITLE_CHARS).collect())
}

/// `Thu Oct 30 21:00:00 +0800 2025`（`created_at` 的写法）。
fn parse_time(text: &str) -> Result<Rfc3339, String> {
    let parsed = jiff::fmt::strtime::parse("%a %b %d %H:%M:%S %z %Y", text)
        .and_then(|t| t.to_timestamp())
        .map_err(|e| format!("`{text}` is not a created_at time: {e}"))?;
    Ok(Rfc3339::from_timestamp(parsed))
}

fn write(out: &Path, name: &str, bytes: &[u8]) -> Result<(), CallFailure> {
    let dest = out.join(name);
    let io = |source| ApiError::Io {
        path: name.to_owned(),
        source,
    };
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    std::fs::write(&dest, bytes).map_err(io)?;
    Ok(())
}

/// 可选的一步出错时：硬错误（网络、限流、被拦、访客身份被拒、写盘）原样报出，其余的把原因记进 `missingReason`。
fn soft(err: ApiError, what: &str) -> Result<String, CallFailure> {
    if err.is_hard() {
        return Err(err.into());
    }
    Ok(format!("{what}：{err}"))
}

/// 一次调用里累积的东西。
struct Run<'a> {
    api: &'a Weibo,
    out: &'a Path,
    wanted: &'a BTreeSet<OutputKind>,
    /// 交来一部分、还缺一部分的输出。
    partial: BTreeSet<OutputKind>,
    /// 这条微博本来就没有的输出，不算缺。
    absent: BTreeSet<OutputKind>,
    reasons: Vec<String>,
    failed: Vec<Value>,
    images: Vec<RelPath>,
    media: Vec<Media>,
    image_records: Vec<Value>,
    video_records: Vec<Value>,
}

impl Run<'_> {
    /// 长文：`isLongText` 的微博再请求一次 `longtext`，用它的正文换掉 `text`。
    /// 取不到时留着 `show` 给的，记 `text` 不完整。
    fn long_text(
        &mut self,
        cookie: &Cookie,
        post: &mut Post,
        file: &str,
        who: &str,
    ) -> Result<(), CallFailure> {
        if !post.is_long_text {
            return Ok(());
        }
        let Some(mblogid) = post.mblogid.clone().or_else(|| mblogid_from_mid(&post.mid)) else {
            self.long_text_failed(who, "没有 mblogid");
            return Ok(());
        };
        let raw = match self.api.longtext(&mblogid, cookie) {
            Ok(raw) => raw,
            Err(e) => {
                let why = soft(e, &format!("{who}长文"))?;
                self.partial.insert(OutputKind::Text);
                self.reasons.push(why);
                return Ok(());
            }
        };
        write(self.out, file, &raw)?;
        let value: Option<Value> = serde_json::from_slice(&raw).ok();
        let data = value
            .as_ref()
            .filter(|v| v.get("ok").and_then(Value::as_i64).is_none_or(|ok| ok == 1))
            .and_then(|v| v.get("data"));
        match data
            .and_then(|d| d.get("longTextContent"))
            .and_then(Value::as_str)
        {
            Some(html) => {
                post.urls.extend(data.map(url_pairs).unwrap_or_default());
                post.text_html = Some(html.to_owned());
            }
            None => self.long_text_failed(who, "回答里没有 longTextContent"),
        }
        Ok(())
    }

    fn long_text_failed(&mut self, who: &str, why: &str) {
        self.partial.insert(OutputKind::Text);
        self.reasons
            .push(format!("{who}长文没有取到（{why}），正文可能只有前一段"));
    }

    /// 下一条微博（`prefix` 为 `""` 或 `q-`）的图片和视频。
    fn download_media(&mut self, post: &Post, prefix: &str) -> Result<(), CallFailure> {
        let who = if prefix.is_empty() { "" } else { "转发的" };
        if self.wanted.contains(&OutputKind::Images) {
            if post.pics_without_info > 0 {
                self.partial.insert(OutputKind::Images);
                self.reasons.push(format!(
                    "{who}有 {} 张图在 pic_infos 里没有信息",
                    post.pics_without_info
                ));
            }
            if let Some(cover) = &post.cover {
                self.image(
                    &format!("images/{prefix}cover"),
                    &format!("{who}封面"),
                    cover,
                )?;
            }
            for (i, pic) in post.pics.iter().enumerate() {
                if self.images.len() >= MAX_IMAGES {
                    self.partial.insert(OutputKind::Images);
                    self.reasons.push(format!("只取了前 {MAX_IMAGES} 张图"));
                    break;
                }
                let stem = format!("images/{prefix}{:03}", i + 1);
                let label = format!("{who}图片 {}", i + 1);
                match &pic.url {
                    Some(url) => {
                        if let Some(path) = self.image(&stem, &label, url)? {
                            self.image_records.push(json!({
                                "path": path.as_str(), "picId": pic.id,
                                "width": pic.width, "height": pic.height,
                            }));
                        }
                    }
                    None => self.image_missing(&label, "回答里没有图片地址"),
                }
            }
        }
        if self.wanted.contains(&OutputKind::Video) {
            if post.videos_without_url > 0 {
                self.partial.insert(OutputKind::Video);
                self.reasons.push(format!(
                    "{who}有 {} 个视频，回答里没有认出地址",
                    post.videos_without_url
                ));
            }
            for (i, video) in post.videos.iter().enumerate() {
                let name = format!("media/{prefix}video-{:03}.mp4", i + 1);
                let label = format!("{who}视频 {}", i + 1);
                if let Some(path) = self.video(&name, &label, &video.url)? {
                    self.video_records.push(json!({
                        "path": path.as_str(), "durationSec": video.duration_sec,
                        "width": video.width, "height": video.height,
                    }));
                    self.media.push(Media {
                        path,
                        kind: MediaKind::Video,
                        mime: Some("video/mp4".into()),
                        lang: None,
                        duration_sec: video.duration_sec,
                        group: None,
                    });
                }
            }
            for (i, pic) in post.pics.iter().enumerate() {
                if let Some(url) = live_video(pic) {
                    let name = format!("media/{prefix}live-{:03}.mp4", i + 1);
                    let label = format!("{who}实况图 {} 的视频", i + 1);
                    if let Some(path) = self.video(&name, &label, url)? {
                        self.media.push(Media {
                            path,
                            kind: MediaKind::Video,
                            mime: Some("video/mp4".into()),
                            lang: None,
                            duration_sec: None,
                            group: None,
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// 下一张图；软错误记进 `partial`、`reasons`，返回 `None`。
    fn image(
        &mut self,
        stem: &str,
        label: &str,
        raw: &str,
    ) -> Result<Option<RelPath>, CallFailure> {
        let done = (|| -> Result<RelPath, ApiError> {
            let url = self.api.endpoints().media_url(raw)?;
            let ext = url
                .path()
                .rsplit_once('.')
                .map(|(_, e)| e.to_ascii_lowercase())
                .filter(|e| matches!(e.as_str(), "jpg" | "jpeg" | "png" | "webp" | "gif" | "avif"))
                .unwrap_or_else(|| "jpg".into());
            let name = format!("{stem}.{ext}");
            step(label);
            self.api.download(
                &url,
                &self.out.join(&name),
                IMAGE_BYTES,
                IMAGE_TIMEOUT,
                label,
            )?;
            RelPath::try_from(name).map_err(ApiError::Shape)
        })();
        match done {
            Ok(path) => {
                self.images.push(path.clone());
                Ok(Some(path))
            }
            Err(e) => {
                let why = soft(e, label)?;
                self.image_missing(label, &why);
                Ok(None)
            }
        }
    }

    fn image_missing(&mut self, label: &str, why: &str) {
        self.partial.insert(OutputKind::Images);
        self.failed.push(json!({"what": label, "why": why}));
        self.reasons.push(format!("{label}：{why}"));
    }

    fn video(
        &mut self,
        name: &str,
        label: &str,
        raw: &str,
    ) -> Result<Option<RelPath>, CallFailure> {
        let done = (|| -> Result<RelPath, ApiError> {
            let url = self.api.endpoints().media_url(raw)?;
            step(label);
            self.api.download(
                &url,
                &self.out.join(name),
                VIDEO_BYTES,
                VIDEO_TIMEOUT,
                label,
            )?;
            RelPath::try_from(name).map_err(ApiError::Shape)
        })();
        match done {
            Ok(path) => Ok(Some(path)),
            Err(e) => {
                let why = soft(e, label)?;
                self.partial.insert(OutputKind::Video);
                self.failed.push(json!({"what": label, "why": why}));
                self.reasons.push(why);
                Ok(None)
            }
        }
    }

    fn meta(&self, post: &Post) -> Value {
        let mut value = post_meta(post);
        let object = value.as_object_mut().expect("post_meta is an object");
        object.insert("images".into(), Value::Array(self.image_records.clone()));
        object.insert("videos".into(), Value::Array(self.video_records.clone()));
        object.insert("failed".into(), Value::Array(self.failed.clone()));
        object.insert("missing".into(), json!(self.reasons));
        value
    }
}

/// 实况图和动图的视频（`type: livephoto`、`gif` 的图带 `video`）。
fn live_video(pic: &Pic) -> Option<&str> {
    pic.video.as_deref()
}

fn post_meta(post: &Post) -> Value {
    let mut value = json!({
        "mid": post.mid,
        "mblogid": post.mblogid,
        "url": crate::ids::canonical_url(&post.mid),
        "author": post.user.as_ref().map(|u| json!({"id": u.id, "screenName": u.name})),
        "createdAt": post.created_at,
        "source": post.source_html.as_deref().map(|s| html_to_text(s, &[])),
        "regionName": post.region,
        "isLongText": post.is_long_text,
        "counts": {
            "reposts": post.counts.reposts,
            "comments": post.counts.comments,
            "attitudes": post.counts.attitudes,
        },
        "topics": post.topics,
    });
    let quoted = match &post.retweet {
        Some(Retweet::Post(q)) => Some(post_meta(q)),
        Some(Retweet::Gone(why)) => Some(json!({"gone": why})),
        None => None,
    };
    if let Some(q) = quoted {
        value["quoted"] = q;
    }
    value
}
