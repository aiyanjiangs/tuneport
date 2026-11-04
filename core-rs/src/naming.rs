//! Windows 文件名清洗、模板拼装、重名处理。

use std::path::{Path, PathBuf};

const RESERVED: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];

pub fn sanitize(name: &str, max_len: usize) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();

    // 折叠连续空白
    s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    // 去掉结尾的点与空格(Windows 会静默丢弃,导致路径对不上)
    while s.ends_with('.') || s.ends_with(' ') {
        s.pop();
    }
    if s.is_empty() {
        return "未命名".to_string();
    }

    // 保留名
    let upper = s.to_uppercase();
    let stem = upper.split('.').next().unwrap_or("");
    let is_reserved = RESERVED.contains(&stem)
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit());
    if is_reserved {
        s = format!("_{}", s);
    }

    if s.chars().count() > max_len {
        s = s.chars().take(max_len).collect();
        while s.ends_with('.') || s.ends_with(' ') {
            s.pop();
        }
    }
    if s.is_empty() {
        "未命名".to_string()
    } else {
        s
    }
}

/// 按模板拼基名,空字段产生的悬空分隔符会被清掉
pub fn build_base_name(
    artist: &str,
    title: &str,
    album: &str,
    template: &str,
    fallback: &str,
) -> String {
    // 标签里多歌手用 " / " 分隔,但文件名里 "/" 是非法字符 —— 落盘时换成逗号
    let artist = if artist.contains(" / ") {
        artist.replace(" / ", ", ")
    } else {
        artist.to_string()
    };

    let mut s = template
        .replace("{artist}", &artist)
        .replace("{title}", title)
        .replace("{album}", album);

    s = s.trim().to_string();
    // 去掉首尾的悬空分隔符
    let trim_chars: &[char] = &[' ', '-', '–', '—'];
    s = s.trim_matches(|c: char| trim_chars.contains(&c)).to_string();

    if s.trim().is_empty() {
        s = if !fallback.is_empty() {
            fallback.to_string()
        } else if !title.is_empty() {
            title.to_string()
        } else {
            "未命名".to_string()
        };
    }
    sanitize(&s, 100)
}

/// 在目录里找一个不冲突的路径(不创建文件)
pub fn unique_path(dir: &Path, base: &str, ext: &str) -> PathBuf {
    let mut candidate = dir.join(format!("{}{}", base, ext));
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{} ({}){}", base, n, ext));
        n += 1;
        if n > 9999 {
            break;
        }
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_cases() {
        assert_eq!(sanitize("林墨: 夏日/信笺?", 100), "林墨_ 夏日_信笺_");
        assert_eq!(sanitize("  结尾有点.  ", 100), "结尾有点");
        assert_eq!(sanitize("CON", 100), "_CON");
        assert_eq!(sanitize("", 100), "未命名");
    }

    #[test]
    fn template_cases() {
        assert_eq!(
            build_base_name("林墨, 苏晚", "夏日信笺", "白日梦游", "{artist} - {title}", "x"),
            "林墨, 苏晚 - 夏日信笺"
        );
        // 没有歌手时不留悬空横杠
        assert_eq!(build_base_name("", "只有歌名", "", "{artist} - {title}", "x"), "只有歌名");
    }
}
