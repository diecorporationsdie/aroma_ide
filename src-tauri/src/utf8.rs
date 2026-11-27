//! Потоковый декодер UTF-8.
//!
//! PTY и пайпы отдают байты произвольными кусками: многобайтовый символ
//! (русская буква, эмодзи) легко разрывается между двумя `read`. Наивный
//! `String::from_utf8_lossy` на каждом чанке превращает такие символы в `�`.
//! Именно из-за этого в терминале с русской локалью появляются «крякозябры».
//!
//! Декодер придерживает неполный хвост до следующего чанка и заменяет только
//! реально битые байты.

/// Максимальная длина UTF-8 последовательности.
const MAX_SEQUENCE: usize = 4;

#[derive(Default)]
pub struct Utf8Stream {
    carry: Vec<u8>,
}

impl Utf8Stream {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Добавляет байты, возвращает максимально декодируемый текст.
    pub fn push(&mut self, chunk: &[u8]) -> String {
        if chunk.is_empty() {
            return String::new();
        }
        let mut buf = std::mem::take(&mut self.carry);
        buf.extend_from_slice(chunk);
        self.decode(&buf)
    }

    /// Сбрасывает накопленный хвост (вызывается по EOF).
    pub fn finish(&mut self) -> String {
        if self.carry.is_empty() {
            return String::new();
        }
        let tail = std::mem::take(&mut self.carry);
        String::from_utf8_lossy(&tail).into_owned()
    }

    fn decode(&mut self, buf: &[u8]) -> String {
        let mut out = String::with_capacity(buf.len());
        let mut rest = buf;

        loop {
            if rest.is_empty() {
                break;
            }
            match std::str::from_utf8(rest) {
                Ok(valid) => {
                    out.push_str(valid);
                    break;
                }
                Err(err) => {
                    let valid_up_to = err.valid_up_to();
                    // Безопасно: граница проверена std.
                    if let Ok(prefix) = std::str::from_utf8(&rest[..valid_up_to]) {
                        out.push_str(prefix);
                    }

                    match err.error_len() {
                        // Реально битая последовательность — заменяем и идём дальше.
                        Some(bad) => {
                            out.push('\u{FFFD}');
                            rest = &rest[valid_up_to + bad..];
                        }
                        // Хвост может оказаться началом символа — придержим.
                        None => {
                            let tail = &rest[valid_up_to..];
                            if tail.len() < MAX_SEQUENCE {
                                self.carry = tail.to_vec();
                            } else {
                                out.push('\u{FFFD}');
                            }
                            break;
                        }
                    }
                }
            }
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_multibyte_across_chunks() {
        let text = "привет, мир";
        let mut stream = Utf8Stream::new();
        let mut out = String::new();
        // Худший случай: по одному байту.
        for b in text.as_bytes() {
            out.push_str(&stream.push(&[*b]));
        }
        out.push_str(&stream.finish());
        assert_eq!(out, text);
    }

    #[test]
    fn ascii_fast_path() {
        let mut stream = Utf8Stream::new();
        assert_eq!(stream.push(b"hello"), "hello");
        assert_eq!(stream.push(b" world"), " world");
        assert_eq!(stream.finish(), "");
    }

    #[test]
    fn replaces_invalid_bytes() {
        let mut stream = Utf8Stream::new();
        let out = stream.push(&[b'a', 0xFF, b'b']);
        assert!(out.starts_with('a'));
        assert!(out.ends_with('b'));
        assert!(out.contains('\u{FFFD}'));
    }

    #[test]
    fn carries_incomplete_tail() {
        let mut stream = Utf8Stream::new();
        // Первый байт «й» (0xD0 0xB9).
        assert_eq!(stream.push(&[0xD0]), "");
        assert_eq!(stream.push(&[0xB9]), "й");
    }

    #[test]
    fn finish_flushes_broken_tail() {
        let mut stream = Utf8Stream::new();
        assert_eq!(stream.push(&[0xD0]), "");
        assert_eq!(stream.finish(), "\u{FFFD}");
    }

    #[test]
    fn emoji_and_crlf() {
        let mut stream = Utf8Stream::new();
        let mut out = String::new();
        let bytes = "🚀ok\r\n".as_bytes();
        for chunk in bytes.chunks(1) {
            out.push_str(&stream.push(chunk));
        }
        out.push_str(&stream.finish());
        assert_eq!(out, "🚀ok\r\n");
    }
}
