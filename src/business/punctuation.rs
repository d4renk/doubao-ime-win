use crate::data::PunctuationMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptBoundary {
    Interim,
    ClauseFinal,
    SessionFinal,
}

pub fn format_transcript(
    text: &str,
    mode: PunctuationMode,
    boundary: TranscriptBoundary,
) -> String {
    match mode {
        PunctuationMode::Smart => match boundary {
            TranscriptBoundary::Interim => strip_sentence_ending(text),
            TranscriptBoundary::ClauseFinal => with_sentence_ending(text, '，'),
            TranscriptBoundary::SessionFinal => with_sentence_ending(text, '。'),
        },
        PunctuationMode::Spaces => punctuation_to_spaces(text),
        PunctuationMode::NoSentenceFinal => strip_sentence_ending(text),
        PunctuationMode::Preserve => text.to_string(),
    }
}

fn with_sentence_ending(text: &str, ending: char) -> String {
    let mut formatted = strip_sentence_ending(text);
    if !formatted.is_empty() {
        formatted.push(ending);
    }
    formatted
}

fn strip_sentence_ending(text: &str) -> String {
    let mut characters: Vec<char> = text.trim_end().chars().collect();
    let mut closing_marks = Vec::new();

    while characters.last().copied().is_some_and(is_closing_mark) {
        closing_marks.push(characters.pop().unwrap());
    }
    while characters.last().copied().is_some_and(is_sentence_mark) {
        characters.pop();
    }
    while characters.last().copied().is_some_and(char::is_whitespace) {
        characters.pop();
    }
    characters.extend(closing_marks.into_iter().rev());
    characters.into_iter().collect()
}

fn punctuation_to_spaces(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut pending_space = false;

    for character in text.chars() {
        if is_punctuation(character) || character.is_whitespace() {
            pending_space = !output.is_empty();
            continue;
        }
        if pending_space {
            output.push(' ');
            pending_space = false;
        }
        output.push(character);
    }
    output
}

fn is_sentence_mark(character: char) -> bool {
    matches!(
        character,
        '，' | '。' | '！' | '？' | '；' | '：' | '、' | '…' | ',' | '.' | '!' | '?' | ';' | ':'
    )
}

fn is_closing_mark(character: char) -> bool {
    matches!(
        character,
        '”' | '’' | '"' | '\'' | ')' | '）' | ']' | '】' | '}' | '》' | '」' | '』'
    )
}

fn is_punctuation(character: char) -> bool {
    character.is_ascii_punctuation()
        || matches!(
            character,
            '\u{3001}'..='\u{303f}'
                | '\u{ff01}'..='\u{ff0f}'
                | '\u{ff1a}'..='\u{ff20}'
                | '\u{ff3b}'..='\u{ff40}'
                | '\u{ff5b}'..='\u{ff65}'
                | '…'
                | '—'
                | '“'
                | '”'
                | '‘'
                | '’'
                | '·'
        )
}

#[cfg(test)]
mod tests {
    use super::{format_transcript, TranscriptBoundary};
    use crate::data::PunctuationMode;

    #[test]
    fn smart_punctuation_follows_voice_session_boundary() {
        assert_eq!(
            format_transcript(
                "你好。",
                PunctuationMode::Smart,
                TranscriptBoundary::Interim
            ),
            "你好"
        );
        assert_eq!(
            format_transcript(
                "你好。",
                PunctuationMode::Smart,
                TranscriptBoundary::ClauseFinal
            ),
            "你好，"
        );
        assert_eq!(
            format_transcript(
                "你好，",
                PunctuationMode::Smart,
                TranscriptBoundary::SessionFinal
            ),
            "你好。"
        );
    }

    #[test]
    fn punctuation_can_be_replaced_with_single_spaces() {
        assert_eq!(
            format_transcript(
                "你好，世界！ Chrome...浏览器",
                PunctuationMode::Spaces,
                TranscriptBoundary::SessionFinal
            ),
            "你好 世界 Chrome 浏览器"
        );
    }

    #[test]
    fn no_final_mode_keeps_internal_punctuation() {
        assert_eq!(
            format_transcript(
                "你好，世界。",
                PunctuationMode::NoSentenceFinal,
                TranscriptBoundary::SessionFinal
            ),
            "你好，世界"
        );
    }

    #[test]
    fn preserve_mode_returns_server_text_unchanged() {
        let text = "你好，world!";
        assert_eq!(
            format_transcript(
                text,
                PunctuationMode::Preserve,
                TranscriptBoundary::SessionFinal
            ),
            text
        );
    }

    #[test]
    fn ending_normalization_preserves_closing_quotes() {
        assert_eq!(
            format_transcript(
                "他说“你好。”",
                PunctuationMode::Smart,
                TranscriptBoundary::SessionFinal
            ),
            "他说“你好”。"
        );
    }
}
