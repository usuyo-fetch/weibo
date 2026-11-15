//! 认链接，并在 `mid`（十进制）和 `mblogid`（base62）之间换算。不发请求。
//!
//! 认的形状：
//! - `weibo.com/<uid>/<mblogid 或 mid>`、`www.weibo.com/<uid>/…`
//! - `weibo.com/detail/<mid 或 mblogid>`
//! - `m.weibo.cn/status/<id>`、`m.weibo.cn/detail/<id>`
//!
//! 身份是十进制的 `mid`：
//! 全是数字（最多 20 位）就当 `mid`、去掉前导 0；否则当 base62 的 `mblogid` 换算。
//! 规范链接也一样：`https://m.weibo.cn/detail/<mid>`。

use url::Url;

pub const WEB_HOST: &str = "weibo.com";

const BASE62: &[u8; 62] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
/// `mblogid` 的每 4 个字符对应 `mid` 的 7 位十进制。
const GROUP_CHARS: usize = 4;
const GROUP_DIGITS: usize = 7;
/// `mblogid` 最长 3 组；`mid` 最长 20 位。
const MAX_MBLOGID: usize = 12;
const MAX_MID: usize = 20;

#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    /// 微博的十进制 `mid`。
    Post {
        mid: String,
    },
    Foreign,
}

pub fn canonical_url(mid: &str) -> String {
    format!("https://m.weibo.cn/detail/{mid}")
}

/// 一条链接（首尾空白不算）里的微博。不是微博的链接回 `Foreign`。
pub fn parse_input(input: &str) -> Parsed {
    let Ok(url) = Url::parse(input.trim()) else {
        return Parsed::Foreign;
    };
    if !matches!(url.scheme(), "http" | "https") || url.username() != "" || url.password().is_some()
    {
        return Parsed::Foreign;
    }
    let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
        return Parsed::Foreign;
    };
    let Some(segments) = url.path_segments() else {
        return Parsed::Foreign;
    };
    let segments: Vec<&str> = segments.filter(|s| !s.is_empty()).collect();
    let id = match (host.as_str(), segments.as_slice()) {
        ("m.weibo.cn", ["status" | "detail", id]) => *id,
        ("weibo.com" | "www.weibo.com", ["detail", id]) => *id,
        ("weibo.com" | "www.weibo.com", [uid, id])
            if !uid.is_empty() && uid.bytes().all(|b| b.is_ascii_digit()) =>
        {
            *id
        }
        _ => return Parsed::Foreign,
    };
    match mid_from_id(id) {
        Some(mid) => Parsed::Post { mid },
        None => Parsed::Foreign,
    }
}

/// 链接里的那一段是 `mid` 或 `mblogid`，换成十进制 `mid`。
pub fn mid_from_id(text: &str) -> Option<String> {
    if !text.is_empty() && text.len() <= MAX_MID && text.bytes().all(|b| b.is_ascii_digit()) {
        return nonzero(text);
    }
    if text.is_empty()
        || text.len() > MAX_MBLOGID
        || !text.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        return None;
    }
    let bytes = text.as_bytes();
    let mut groups: Vec<String> = Vec::new();
    let mut end = bytes.len();
    while end > 0 {
        let start = end.saturating_sub(GROUP_CHARS);
        let mut n: u64 = 0;
        for &b in &bytes[start..end] {
            n = n * 62 + BASE62.iter().position(|&c| c == b)? as u64;
        }
        // 最左一组不补 0
        groups.push(if start == 0 {
            n.to_string()
        } else {
            format!("{n:0GROUP_DIGITS$}")
        });
        end = start;
    }
    groups.reverse();
    nonzero(&groups.concat())
}

/// `mid` 换成 `mblogid`（`longtext` 接口认它）。`mid` 不是十进制数字时回 `None`。
pub fn mblogid_from_mid(mid: &str) -> Option<String> {
    if mid.is_empty() || mid.len() > MAX_MID || !mid.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut groups: Vec<&str> = Vec::new();
    let mut end = mid.len();
    while end > 0 {
        let start = end.saturating_sub(GROUP_DIGITS);
        groups.push(&mid[start..end]);
        end = start;
    }
    groups.reverse();
    let last = groups.len() - 1;
    let mut out = String::new();
    for (i, group) in groups.iter().enumerate() {
        let mut n: u64 = group.parse().ok()?;
        let mut chars = Vec::new();
        while n > 0 {
            chars.push(BASE62[(n % 62) as usize]);
            n /= 62;
        }
        // 最左一组不补位，其余补到 4 个字符
        let width = if i == 0 { 0 } else { GROUP_CHARS };
        while chars.len() < width {
            chars.push(b'0');
        }
        if chars.is_empty() && i == 0 && last == 0 {
            chars.push(b'0');
        }
        chars.reverse();
        out.push_str(std::str::from_utf8(&chars).ok()?);
    }
    Some(out)
}

fn nonzero(digits: &str) -> Option<String> {
    let trimmed = digits.trim_start_matches('0');
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    // 公开资料里常引的一对
    const BID: &str = "z0JH2lOMb";
    const MID: &str = "3501756485200075";

    #[test]
    fn mblogid_and_mid_convert_both_ways() {
        assert_eq!(mid_from_id(BID).as_deref(), Some(MID));
        assert_eq!(mblogid_from_mid(MID).as_deref(), Some(BID));
        assert_eq!(mid_from_id(MID).as_deref(), Some(MID));
        assert_eq!(mid_from_id("bad!"), None);
        assert_eq!(mid_from_id(""), None);
        assert_eq!(mid_from_id("0000"), None);
        assert_eq!(mid_from_id("0012345").as_deref(), Some("12345"));
        assert_eq!(mid_from_id("1234567890123456789012"), None);
    }

    #[test]
    fn round_trip_over_many_mids() {
        // 含 7 位一组里出现前导 0 的写法
        for mid in [
            "4900000000000001",
            "4900000000000002",
            "5120561132606436",
            "4189191225395228",
            "4910815147462302",
            "3501756400000075",
            "1000000",
            "9999999",
        ] {
            let bid = mblogid_from_mid(mid).unwrap();
            assert_eq!(mid_from_id(&bid).as_deref(), Some(mid), "{mid} -> {bid}");
        }
    }

    #[test]
    fn link_shapes() {
        let mid = |s: &str| match parse_input(s) {
            Parsed::Post { mid } => Some(mid),
            Parsed::Foreign => None,
        };
        for link in [
            format!("https://weibo.com/1700000001/{BID}"),
            format!("https://www.weibo.com/1700000001/{BID}?refer_flag=1"),
            format!("https://weibo.com/0/{MID}"),
            format!("https://weibo.com/detail/{MID}"),
            format!("https://www.weibo.com/detail/{BID}"),
            format!("https://m.weibo.cn/status/{MID}"),
            format!("https://m.weibo.cn/status/{BID}"),
            format!("https://m.weibo.cn/detail/{MID}"),
            format!("  http://m.weibo.cn/detail/{MID}\n"),
        ] {
            assert_eq!(mid(&link).as_deref(), Some(MID), "{link}");
        }
        for link in [
            "https://weibo.com/u/1700000001",
            "https://weibo.com/u/page/fav/1700000001",
            "https://weibo.com/ttarticle/p/show?id=2309405312350592041114",
            "https://weibo.com/tv/show/1034:4797699866951785",
            "https://m.weibo.cn/u/1700000001",
            "https://example.com/1700000001/z0JH2lOMb",
            "https://weibo.com.evil.test/1700000001/z0JH2lOMb",
            "https://user:pw@weibo.com/1700000001/z0JH2lOMb",
            "ftp://weibo.com/1700000001/z0JH2lOMb",
            "https://weibo.com/1700000001/bad!",
            "not a link",
        ] {
            assert_eq!(mid(link), None, "{link}");
        }
    }
}
