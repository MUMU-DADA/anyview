//! 文件名自然排序：数字按数值比较，其余按字符比较且不区分大小写。
//! 例：1, 2, 10, a, B, img2, IMG3, img10, 中文。
//! 非 ASCII 字符暂按码位比较，与访达按语言（如拼音）的顺序可能不同。

use std::cmp::Ordering;
use std::iter::Peekable;
use std::str::Chars;

/// 比较两个文件名。完全"相同"（只差大小写或前导零）时，再按原始字符串比较，保证顺序稳定。
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    compare_chunks(a, b).then_with(|| a.cmp(b))
}

fn compare_chunks(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let da = take_digits(&mut ai);
                    let db = take_digits(&mut bi);
                    let ta = da.trim_start_matches('0');
                    let tb = db.trim_start_matches('0');
                    let ord = ta
                        .len()
                        .cmp(&tb.len())
                        .then_with(|| ta.cmp(tb))
                        .then_with(|| da.len().cmp(&db.len()));
                    if ord != Ordering::Equal {
                        return ord;
                    }
                } else {
                    let ord = fold(ca).cmp(&fold(cb));
                    if ord != Ordering::Equal {
                        return ord;
                    }
                    ai.next();
                    bi.next();
                }
            }
        }
    }
}

fn take_digits(chars: &mut Peekable<Chars<'_>>) -> String {
    let mut digits = String::new();
    while let Some(&c) = chars.peek() {
        if !c.is_ascii_digit() {
            break;
        }
        digits.push(c);
        chars.next();
    }
    digits
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_like_a_file_manager() {
        let mut names = vec![
            "10.png",
            "1.png",
            "2.png",
            "img10.png",
            "img2.png",
            "IMG3.png",
            "a.png",
            "B.png",
            "中文.png",
        ];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            names,
            [
                "1.png",
                "2.png",
                "10.png",
                "a.png",
                "B.png",
                "img2.png",
                "IMG3.png",
                "img10.png",
                "中文.png"
            ]
        );
    }

    #[test]
    fn leading_zeros_and_case_only_break_ties() {
        assert_eq!(natural_cmp("img01.png", "img1.png"), Ordering::Greater);
        assert_eq!(natural_cmp("img001.png", "img2.png"), Ordering::Less);
        assert_ne!(natural_cmp("A.png", "a.png"), Ordering::Equal);
        assert_eq!(natural_cmp("a.png", "a1.png"), Ordering::Less);
    }

    #[test]
    fn very_long_numbers_do_not_overflow() {
        assert_eq!(
            natural_cmp(
                "x99999999999999999999999.png",
                "x100000000000000000000000.png"
            ),
            Ordering::Less
        );
    }
}
