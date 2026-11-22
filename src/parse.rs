//! 指令列解析: 支援 `'...'`、`"..."` 引號與 `\` 跳脫。

#[derive(Clone, Copy, PartialEq, Eq)]
enum Quote {
    None,
    Single,
    Double,
}

/// 把一行指令切成參數。
///
/// - `'...'` 內的內容原樣保留
/// - `"..."` 內可用 `\` 跳脫 `"` 與 `\`
/// - 引號外可用 `\` 跳脫任何字元(例如 `\ ` 表示空白)
/// - 沒有關閉的引號視為延續到行尾
pub fn split_args(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    // 用來區分 `""`(空字串參數)與沒有參數
    let mut has_token = false;
    let mut quote = Quote::None;
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        match (quote, c) {
            (Quote::None, c) if c.is_whitespace() => {
                if has_token {
                    args.push(std::mem::take(&mut cur));
                    has_token = false;
                }
            }
            (Quote::None, '\'') => {
                quote = Quote::Single;
                has_token = true;
            }
            (Quote::None, '"') => {
                quote = Quote::Double;
                has_token = true;
            }
            (Quote::None, '\\') => {
                has_token = true;
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            (Quote::Single, '\'') | (Quote::Double, '"') => quote = Quote::None,
            (Quote::Double, '\\') if matches!(chars.peek(), Some('"' | '\\')) => {
                cur.push(chars.next().unwrap());
            }
            (_, c) => {
                has_token = true;
                cur.push(c);
            }
        }
    }
    if has_token {
        args.push(cur);
    }
    args
}

/// 文字結尾是否還在引號內(例如 `echo "what` 回傳 true)
pub fn in_quote(text: &str) -> bool {
    let mut quote = Quote::None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        quote = match (quote, c) {
            (Quote::None, '\'') => Quote::Single,
            (Quote::None, '"') => Quote::Double,
            (Quote::None | Quote::Double, '\\') => {
                chars.next();
                quote
            }
            (Quote::Single, '\'') | (Quote::Double, '"') => Quote::None,
            (q, _) => q,
        };
    }
    quote != Quote::None
}

/// 必要時加上引號, 讓 [`split_args`] 能還原成同一個參數
pub fn quote_arg(arg: &str) -> String {
    let special = |c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '\\');
    if !arg.is_empty() && !arg.chars().any(special) {
        return arg.to_string();
    }
    let escaped = arg.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split() {
        assert_eq!(split_args("echo  a b"), ["echo", "a", "b"]);
        assert_eq!(split_args(r#"echo "what?  x""#), ["echo", "what?  x"]);
        assert_eq!(split_args("echo 'it''s'"), ["echo", "its"]);
        assert_eq!(split_args(r#"echo "say \"hi\"""#), ["echo", r#"say "hi""#]);
        assert_eq!(split_args(r"echo a\ b"), ["echo", "a b"]);
        assert_eq!(split_args(r#"echo """#), ["echo", ""]);
        assert_eq!(split_args(r#"echo "unclosed  x"#), ["echo", "unclosed  x"]);
        assert_eq!(split_args("  "), Vec::<String>::new());
    }

    #[test]
    fn quote_roundtrip() {
        for arg in ["abc", "a b", "", r#"say "hi""#, "it's", r"a\b", "中文 字"] {
            let q = quote_arg(arg);
            assert_eq!(split_args(&format!("echo {q}")), ["echo", arg], "{q}");
        }
        assert_eq!(quote_arg("abc"), "abc");
    }

    #[test]
    fn quote_state() {
        assert!(!in_quote("echo what"));
        assert!(in_quote("echo 'what"));
        assert!(in_quote(r#"echo "what"#));
        assert!(!in_quote(r#"echo "what""#));
        assert!(in_quote(r#"echo "it's"#));
        assert!(!in_quote(r#"echo it\'s"#));
        assert!(in_quote(r#"echo "a \" b"#));
    }
}
