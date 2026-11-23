//! What the CLI does before it starts sharing or joining, given whether the
//! terms are stored, whether `--accept-terms` was passed, and whether a person
//! can answer (stdin is a terminal). See jafnify_core::terms for the text and
//! the stored acceptance; the terminal check stays in the platform.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// Already accepted: start.
    Proceed,
    /// `--accept-terms` given: store the acceptance, then start.
    Record,
    /// Print the terms and ask y/N; a yes stores the acceptance.
    Ask,
    /// Not accepted and nobody can answer: do not start.
    Refuse,
}

pub fn cli_gate(accepted: bool, accept_flag: bool, interactive: bool) -> Gate {
    if accepted {
        Gate::Proceed
    } else if accept_flag {
        Gate::Record
    } else if interactive {
        Gate::Ask
    } else {
        Gate::Refuse
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_accepted_user_proceeds_without_a_prompt() {
        assert_eq!(cli_gate(true, false, false), Gate::Proceed);
        assert_eq!(cli_gate(true, false, true), Gate::Proceed);
    }

    #[test]
    fn the_flag_records_and_a_terminal_asks() {
        assert_eq!(cli_gate(false, true, false), Gate::Record);
        assert_eq!(cli_gate(false, false, true), Gate::Ask);
        assert_eq!(cli_gate(false, false, false), Gate::Refuse);
    }
}
