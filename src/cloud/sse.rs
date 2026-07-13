use super::CloudError;

#[derive(Debug, Eq, PartialEq)]
pub(super) struct SseEvent {
    pub event: String,
    pub data: String,
}

#[derive(Default)]
pub(super) struct SseDecoder {
    buffer: Vec<u8>,
    event: String,
    data: Vec<String>,
}

impl SseDecoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<SseEvent>, CloudError> {
        self.buffer.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some(newline) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let mut line: Vec<_> = self.buffer.drain(..=newline).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = std::str::from_utf8(&line).map_err(|error| {
                CloudError::InvalidResponse(format!("invalid SSE UTF-8: {error}"))
            })?;
            if let Some(event) = self.line(line) {
                events.push(event);
            }
        }
        Ok(events)
    }

    pub fn finish(mut self) -> Result<Vec<SseEvent>, CloudError> {
        let mut events = Vec::new();
        if !self.buffer.is_empty() {
            let line = std::str::from_utf8(&self.buffer)
                .map_err(|error| {
                    CloudError::InvalidResponse(format!("invalid SSE UTF-8: {error}"))
                })?
                .trim_end_matches('\r')
                .to_owned();
            self.buffer.clear();
            if let Some(event) = self.line(&line) {
                events.push(event);
            }
        }
        if let Some(event) = self.dispatch() {
            events.push(event);
        }
        Ok(events)
    }

    fn line(&mut self, line: &str) -> Option<SseEvent> {
        if line.is_empty() {
            return self.dispatch();
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => self.event = value.to_owned(),
            "data" => self.data.push(value.to_owned()),
            _ => {}
        }
        None
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        if self.data.is_empty() {
            self.event.clear();
            return None;
        }
        Some(SseEvent {
            event: std::mem::take(&mut self.event),
            data: std::mem::take(&mut self.data).join("\n"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_split_utf8_crlf_and_multiple_data_lines() {
        let bytes = "event: scene.delta\r\ndata: {\"content\":\"你".as_bytes();
        let split = bytes.len() - 1;
        let mut decoder = SseDecoder::default();
        assert!(decoder.push(&bytes[..split]).unwrap().is_empty());
        assert!(decoder.push(&bytes[split..]).unwrap().is_empty());
        let events = decoder
            .push("好\"}\r\ndata: tail\r\n\r\n".as_bytes())
            .unwrap();
        assert_eq!(
            events,
            vec![SseEvent {
                event: "scene.delta".into(),
                data: "{\"content\":\"你好\"}\ntail".into(),
            }]
        );
    }

    #[test]
    fn dispatches_last_event_at_eof_without_blank_line() {
        let mut decoder = SseDecoder::default();
        decoder
            .push(b"event:done\ndata:[DONE]")
            .expect("valid prefix");
        assert_eq!(
            decoder.finish().unwrap(),
            vec![SseEvent {
                event: "done".into(),
                data: "[DONE]".into(),
            }]
        );
    }
}
