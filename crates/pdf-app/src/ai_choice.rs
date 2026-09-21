#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Choice {
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub effort: String,
    pub mode: String,
}

pub const DEFAULT_MODE: &str = "ask_before_changes";

const FIELDS: usize = 5;

#[must_use]
pub fn read(text: &str) -> Option<Choice> {
    let line = text.lines().next()?;
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.len() < 3 || fields.len() > FIELDS {
        return None;
    }
    let field = |at: usize| (*fields.get(at).unwrap_or(&"")).to_owned();
    if fields[0].is_empty() {
        return None;
    }
    Some(Choice {
        provider: field(0),
        base_url: field(1),
        model: field(2),
        effort: field(3),
        mode: field(4),
    })
}

#[must_use]
pub fn write(choice: &Choice) -> Option<String> {
    let fields = [
        &choice.provider,
        &choice.base_url,
        &choice.model,
        &choice.effort,
        &choice.mode,
    ];
    if fields
        .iter()
        .any(|field| field.contains(['\t', '\n', '\r']) || field.contains('\u{0}'))
    {
        return None;
    }
    if choice.provider.is_empty() {
        return None;
    }
    Some(format!(
        "{}\t{}\t{}\t{}\t{}\n",
        choice.provider, choice.base_url, choice.model, choice.effort, choice.mode
    ))
}

#[cfg(test)]
mod tests {
    use super::{Choice, DEFAULT_MODE, read, write};

    fn ollama() -> Choice {
        Choice {
            provider: "Ollama".to_owned(),
            base_url: "http://127.0.0.1:11434/v1".to_owned(),
            model: "llama3".to_owned(),
            effort: "medium".to_owned(),
            mode: DEFAULT_MODE.to_owned(),
        }
    }

    #[test]
    fn a_choice_written_reads_back_as_itself() {
        let line = write(&ollama()).expect("it can be written");
        assert_eq!(read(&line), Some(ollama()));
    }

    #[test]
    fn a_choice_with_no_model_is_kept() {
        let choice = Choice {
            model: String::new(),
            ..ollama()
        };
        let line = write(&choice).expect("it can be written");
        assert_eq!(read(&line), Some(choice));
    }

    #[test]
    fn a_line_from_before_the_last_two_fields_still_reads() {
        assert_eq!(
            read("Ollama\thttp://127.0.0.1:11434/v1\tllama3\n"),
            Some(Choice {
                effort: String::new(),
                mode: String::new(),
                ..ollama()
            })
        );
        assert_eq!(
            read("Ollama\thttp://127.0.0.1:11434/v1\tllama3\thigh\n"),
            Some(Choice {
                effort: "high".to_owned(),
                mode: String::new(),
                ..ollama()
            })
        );
    }

    #[test]
    fn what_cannot_be_read_faithfully_is_not_read_or_written() {
        assert_eq!(read(""), None);
        assert_eq!(read("Ollama\thttp://x\n"), None);
        assert_eq!(read("Ollama\thttp://x\tm\tlow\task\tsixth\n"), None);
        assert_eq!(read("\thttp://x\tm\n"), None);
        assert_eq!(
            write(&Choice {
                provider: "Oll\tama".to_owned(),
                ..ollama()
            }),
            None
        );
        assert_eq!(
            write(&Choice {
                provider: String::new(),
                ..ollama()
            }),
            None
        );
    }
}
