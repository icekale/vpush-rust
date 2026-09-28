pub fn to_simplified(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    zhconv::zhconv(text, zhconv::Variant::ZhCN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_traditional_and_leaves_simplified() {
        assert_eq!(to_simplified("漢字轉簡體"), "汉字转简体");
        assert_eq!(to_simplified("汉字转简体"), "汉字转简体");
        assert_eq!(to_simplified("Revenue grew"), "Revenue grew");
        assert_eq!(to_simplified(""), "");
    }
}
