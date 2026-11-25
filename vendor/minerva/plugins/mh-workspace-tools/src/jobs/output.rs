use std::collections::VecDeque;
const CAPACITY: usize = 64 * 1024;
pub(super) const UI_TAIL: usize = 8192;
#[derive(Default)]
pub(super) struct Output {
    bytes: VecDeque<u8>,
    start: u64,
}
impl Output {
    pub fn append(&mut self, bytes: &[u8]) {
        self.bytes.extend(bytes);
        let excess = self.bytes.len().saturating_sub(CAPACITY);
        self.bytes.drain(..excess);
        self.start += excess as u64;
    }
    pub fn cursor(&self) -> u64 {
        self.start + self.bytes.len() as u64
    }
    pub fn read(&self, cursor: u64, limit: usize, settled: bool) -> (String, u64, bool) {
        let mut from = cursor.max(self.start).min(self.cursor());
        if from == self.start && self.start != 0 {
            while self.bytes.get((from - self.start) as usize).is_some_and(|b| b & 0xc0 == 0x80) {
                from += 1;
            }
        }
        let bytes: Vec<_> =
            self.bytes.iter().skip((from - self.start) as usize).take(limit.saturating_add(3)).copied().collect();
        let mut end = 0;
        let mut encoded = 0;
        while end < bytes.len() && end < limit {
            let first = bytes[end];
            let size = match first {
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                _ => 1,
            };
            if end + size > bytes.len() && !settled {
                break;
            }
            let valid = end + size <= bytes.len() && std::str::from_utf8(&bytes[end..end + size]).is_ok();
            let size = if valid { size } else { 1 };
            let cost = if valid { size } else { 3 };
            if end != 0 && (end + size > limit || encoded + cost > limit) {
                break;
            }
            end += size;
            encoded += cost;
        }
        (String::from_utf8_lossy(&bytes[..end]).into_owned(), from + end as u64, cursor < from)
    }
    pub fn tail(&self, settled: bool) -> (String, bool) {
        let from = self.cursor().saturating_sub(UI_TAIL as u64).max(self.start);
        let mut aligned = from;
        while self.bytes.get((aligned - self.start) as usize).is_some_and(|b| b & 0xc0 == 0x80) {
            aligned += 1;
        }
        let (text, _, _) = self.read(aligned, UI_TAIL * 3, settled);
        let mut start = text.len().saturating_sub(UI_TAIL);
        while !text.is_char_boundary(start) {
            start += 1;
        }
        (text[start..].to_owned(), aligned != 0 || start != 0)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tail_keeps_latest_output_after_invalid_bytes_within_encoded_budget() {
        let mut output = Output::default();
        output.append(&vec![0xff; UI_TAIL]);
        output.append(b"latest-marker");
        let (text, truncated) = output.tail(false);
        assert!(text.len() <= UI_TAIL);
        assert!(text.ends_with("latest-marker"));
        assert!(truncated);
    }

    #[test]
    fn incomplete_unicode_is_deferred_without_advancing_model_cursor() {
        let mut output = Output::default();
        output.append(&[0xe2, 0x82]);
        assert_eq!(output.read(0, 1, false), (String::new(), 0, false));
        output.append(&[0xac, b'x']);
        assert_eq!(output.read(0, 1, false), ("€".into(), 3, false));
        assert_eq!(output.read(3, 1, false), ("x".into(), 4, false));
        output.append(&[0xff, 0xe2]);
        assert_eq!(output.read(4, 8, true), ("��".into(), 6, false));
    }
}
