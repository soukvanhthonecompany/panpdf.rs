use std::io::Write;

pub struct Trace {
    file: std::fs::File,
    started: std::time::Instant,
    events: u64,
}

impl Trace {
    pub fn from_env() -> Option<Self> {
        let path = std::env::var_os("PANPDF_TRACE")?;
        match std::fs::File::create(&path) {
            Ok(file) => {
                let mut trace = Self {
                    file,
                    started: std::time::Instant::now(),
                    events: 0,
                };
                trace.line(&format!(
                    r#"{{"record":"open","path":{},"pid":{}}}"#,
                    pdf_app::ledger::quoted(&path.to_string_lossy()),
                    std::process::id()
                ));
                Some(trace)
            }
            Err(error) => {
                eprintln!(
                    "PANPDF_TRACE: {} could not be written: {error}",
                    path.to_string_lossy()
                );
                None
            }
        }
    }

    fn line(&mut self, body: &str) {
        let _ = writeln!(self.file, "{body}");
        let _ = self.file.flush();
    }

    fn seconds(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    pub fn event(&mut self, frame: u64, kind: &str, value: &str, context: &str) {
        self.events += 1;
        let line = format!(
            r#"{{"record":"event","seq":{},"frame":{frame},"t":{:.4},"kind":{},"value":{},"context":{}}}"#,
            self.events,
            self.seconds(),
            pdf_app::ledger::quoted(kind),
            pdf_app::ledger::quoted(value),
            pdf_app::ledger::quoted(context),
        );
        self.line(&line);
    }

    pub fn commands(&mut self, frame: u64, editor: &mut pdf_app::Editor) {
        for record in editor.take_records() {
            let body = record.json();
            let line = format!(
                r#"{{"frame":{frame},"t":{:.4},{}"#,
                self.seconds(),
                &body[1..]
            );
            self.line(&line);
        }
    }

    pub fn note(&mut self, frame: u64, what: &str, detail: &str) {
        let line = format!(
            r#"{{"record":"note","frame":{frame},"t":{:.4},"what":{},"detail":{}}}"#,
            self.seconds(),
            pdf_app::ledger::quoted(what),
            pdf_app::ledger::quoted(detail),
        );
        self.line(&line);
    }
}
