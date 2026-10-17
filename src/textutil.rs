//! 处理外部输入文本的小工具：所有切片都落在字符边界上，不会因多字节字符 panic。

/// `%XX` 解码。按字节处理，`%` 后面不是两位十六进制（包括紧跟中文）时原样保留。
pub fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = |at: usize| bytes.get(at).and_then(|b| (*b as char).to_digit(16));
            if let (Some(high), Some(low)) = (hex(i + 1), hex(i + 2)) {
                out.push((high * 16 + low) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 不大于 `index` 的最近字符边界（`index` 超出长度时返回长度）。
pub fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decode_never_panics_on_multibyte_input() {
        assert_eq!(percent_decode("%中"), "%中");
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(percent_decode("%e4%b8%ad%E6%96%87"), "中文");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%%41"), "%A");
        assert_eq!(percent_decode("%4中"), "%4中");
    }

    #[test]
    fn floor_char_boundary_steps_back_inside_multibyte_chars() {
        let text = "ab中文";
        assert_eq!(floor_char_boundary(text, 3), 2);
        assert_eq!(floor_char_boundary(text, 5), 5);
        assert_eq!(floor_char_boundary(text, 99), text.len());
        assert_eq!(&text[floor_char_boundary(text, 4)..], "中文");
    }
}
