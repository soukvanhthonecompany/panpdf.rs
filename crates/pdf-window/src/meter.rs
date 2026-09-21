use std::io::Write;

#[derive(Default)]
pub struct Frame {
    spans: Vec<(&'static str, f64)>,
    counts: Vec<(&'static str, usize)>,
    notes: Vec<String>,
}

pub struct Meter {
    file: std::fs::File,
    started: std::time::Instant,
    frame: Frame,
}

impl Meter {
    pub fn from_env() -> Option<Self> {
        let path = std::env::var_os("PANPDF_METER")?;
        match std::fs::File::create(&path) {
            Ok(file) => Some(Self {
                file,
                started: std::time::Instant::now(),
                frame: Frame::default(),
            }),
            Err(error) => {
                eprintln!(
                    "PANPDF_METER: {} could not be written: {error}",
                    path.to_string_lossy()
                );
                None
            }
        }
    }

    pub fn span(&mut self, name: &'static str, took: std::time::Duration) {
        self.frame.spans.push((name, took.as_secs_f64() * 1e3));
    }

    pub fn count(&mut self, name: &'static str, many: usize) {
        self.frame.counts.push((name, many));
    }

    pub fn note(&mut self, said: &str) {
        self.frame.notes.push(said.to_owned());
    }

    pub fn end(&mut self, frame: u64, whole: std::time::Duration) {
        let spans = self
            .frame
            .spans
            .iter()
            .map(|(name, took)| format!(r#""{name}":{took:.3}"#))
            .collect::<Vec<_>>()
            .join(",");
        let counts = self
            .frame
            .counts
            .iter()
            .map(|(name, many)| format!(r#""{name}":{many}"#))
            .collect::<Vec<_>>()
            .join(",");
        let notes = self
            .frame
            .notes
            .iter()
            .map(|said| pdf_app::ledger::quoted(said))
            .collect::<Vec<_>>()
            .join(",");
        let line = format!(
            r#"{{"frame":{frame},"t":{:.4},"ms":{:.3},"spans":{{{spans}}},"counts":{{{counts}}},"notes":[{notes}]}}"#,
            self.started.elapsed().as_secs_f64(),
            whole.as_secs_f64() * 1e3,
        );
        let _ = writeln!(self.file, "{line}");
        self.frame = Frame::default();
    }
}
