//! 微博正文是一小段 HTML（`text`、`longTextContent`）：`<br />` 换行，`<a>` 是话题、@、链接，
//! `<img alt="[名字]">` 是表情。转成纯文字：换行照留，表情写成 alt，话题和 @ 留标签上的字，
//! 其余的链接换成它指向的地址（`url_struct` 里有 `short_url` 对应的 `long_url` 时用长的），
//! 其余标签去掉，实体还原。

/// `url_struct` 里的一项：`t.cn` 短链和它指向的地址。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlPair {
    pub short: String,
    pub long: String,
}

pub fn html_to_text(html: &str, urls: &[UrlPair]) -> String {
    let mut out = String::new();
    // 正在读的 <a>：href 和标签里的字从 `label_from` 起
    let mut anchor: Option<(Option<String>, usize)> = None;
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        out.push_str(&decode(&rest[..lt]));
        rest = &rest[lt..];
        let Some(gt) = rest.find('>') else {
            // 没有结尾的 `<`：当成文字
            out.push_str(&decode(rest));
            rest = "";
            break;
        };
        let tag = &rest[1..gt];
        rest = &rest[gt + 1..];
        let (name, attrs) = split_tag(tag);
        match name.as_str() {
            "br" | "/p" | "/div" => out.push('\n'),
            "img" => {
                if let Some(alt) = attr(attrs, "alt") {
                    out.push_str(&decode(&alt));
                }
            }
            "a" => anchor = Some((attr(attrs, "href"), out.len())),
            "/a" => {
                if let Some((href, from)) = anchor.take() {
                    let label = out[from..].to_owned();
                    if let Some(target) = link_target(href.as_deref(), &label, urls) {
                        out.truncate(from);
                        out.push_str(&target);
                    }
                }
            }
            _ => {}
        }
    }
    out.push_str(&decode(rest));
    out.trim().to_owned()
}

/// `<a>` 里的字要不要换成地址：话题、@ 和没有绝对地址的（站内相对链接）留字，其余换成地址。
fn link_target(href: Option<&str>, label: &str, urls: &[UrlPair]) -> Option<String> {
    let href = decode(href?.trim());
    let label = label.trim();
    if label.starts_with('@') || label.starts_with('#') || label.ends_with('#') {
        return None;
    }
    let absolute = if let Some(rest) = href.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        href.clone()
    };
    if !(absolute.starts_with("http://") || absolute.starts_with("https://")) {
        return None;
    }
    let long = urls
        .iter()
        .find(|p| p.short == absolute || p.short == href)
        .map(|p| p.long.clone());
    Some(long.filter(|l| !l.is_empty()).unwrap_or(absolute))
}

/// `a href="x" rel=...` 拆成标签名（小写）和属性串。
fn split_tag(tag: &str) -> (String, &str) {
    let tag = tag.trim();
    let end = tag
        .find(|c: char| c.is_whitespace() || c == '/')
        .filter(|&i| i > 0)
        .unwrap_or(tag.len());
    let (name, attrs) = tag.split_at(end);
    // `/br` 之类的结束标签：斜杠在开头，名字连着斜杠
    if name.is_empty() {
        return (String::new(), attrs);
    }
    (name.to_ascii_lowercase(), attrs)
}

/// 属性值，单引号、双引号都认；没有引号的读到空白为止。
fn attr(attrs: &str, name: &str) -> Option<String> {
    let lower = attrs.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let start = from + i;
        let before_ok = start == 0 || !lower.as_bytes()[start - 1].is_ascii_alphanumeric();
        let after = &attrs[start + name.len()..];
        let after_trim = after.trim_start();
        if before_ok && let Some(value) = after_trim.strip_prefix('=') {
            let value = value.trim_start();
            return Some(match value.chars().next() {
                Some(q @ ('"' | '\'')) => value[1..].split(q).next().unwrap_or_default().to_owned(),
                _ => value
                    .split(|c: char| c.is_whitespace() || c == '>')
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
            });
        }
        from = start + name.len();
    }
    None
}

/// HTML 实体：常见的命名实体和数字实体；认不出的原样留着。
fn decode(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest.find(';').filter(|&s| s <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            _ => entity.strip_prefix('#').and_then(|n| {
                let code = match n.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                    None => n.parse().ok()?,
                };
                char::from_u32(code)
            }),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs() -> Vec<UrlPair> {
        vec![UrlPair {
            short: "http://t.cn/A6exampleA".into(),
            long: "https://example.invalid/long/page?a=1&b=2".into(),
        }]
    }

    #[test]
    fn plain_text_from_html() {
        let html = r#"示例 <br />第二行 <a href="https://weibo.com/n/某人" target="_blank">@某人</a> <a href="https://s.weibo.com/weibo?q=%23话题%23">#示例话题#</a> <a href="http://t.cn/A6exampleA" data-url="http://t.cn/A6exampleA"><span class="url-icon"><img alt="[doge]" src="x"></span><span class="surl-text">网页链接</span></a> &amp; &lt;b&gt; &#20013;&#x6587; <a href="http://t.cn/A6other">其他</a>"#;
        assert_eq!(
            html_to_text(html, &pairs()),
            "示例 \n第二行 @某人 #示例话题# https://example.invalid/long/page?a=1&b=2 & <b> 中文 http://t.cn/A6other"
        );
    }

    #[test]
    fn relative_links_keep_their_label_and_stray_brackets_survive() {
        assert_eq!(
            html_to_text(
                r#"前文…<a href="/status/4900000000000001">全文</a> 1 < 2"#,
                &[]
            ),
            "前文…全文 1 < 2"
        );
        assert_eq!(html_to_text("a<br>b<br/>c</p>", &[]), "a\nb\nc");
        assert_eq!(html_to_text("&unknown; &amp", &[]), "&unknown; &amp");
    }
}
