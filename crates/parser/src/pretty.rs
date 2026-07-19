use cherubsh_common::{
    CASEPAT_FALLTHROUGH, CASEPAT_TESTNEXT, CMD_INVERT_RETURN, CMD_TIME_PIPELINE, CMD_TIME_POSIX,
};

use crate::{
    ArithForCommand, Ast, CaseCommand, Command, CommandData, CondCommand, CondType, Redirect,
    RedirectInstruction, Redirectee, Redirector, WordDesc, CONN_AMP, CONN_AND_AND, CONN_BAR_AND,
    CONN_NEWLINE, CONN_OR_OR, CONN_PIPE, CONN_SEMI,
};

const INDENT: usize = 4;

/// Render a parsed command in Bash's pretty-print format.
///
/// The output comes from the AST, so it omits comments and insignificant
/// whitespace in the same places as Bash's `--pretty-print` mode.
pub fn pretty_print(ast: &Ast) -> String {
    let mut printer = Printer::default();
    printer.command(&ast.root, 0, false);
    while printer.output.ends_with([' ', '\t']) {
        printer.output.pop();
    }
    if !printer.output.ends_with('\n') {
        printer.output.push('\n');
    }
    if is_compound(last_command(&ast.root)) && !printer.output.ends_with("\n\n") {
        printer.output.push('\n');
    }
    printer.output
}

#[derive(Default)]
struct Printer {
    output: String,
}

impl Printer {
    fn command(&mut self, command: &Command, indent: usize, statement: bool) {
        if command.flags & CMD_TIME_PIPELINE != 0 {
            self.indent(indent);
            self.output.push_str("time");
            if command.flags & CMD_TIME_POSIX != 0 {
                self.output.push_str(" -p");
            }
            self.output.push(' ');
        }
        if command.flags & CMD_INVERT_RETURN != 0 {
            if !self.at_line_start() && !self.output.ends_with(' ') {
                self.output.push(' ');
            } else {
                self.indent(indent);
            }
            self.output.push_str("! ");
        }

        match &command.data {
            CommandData::Connection(connection) => {
                let connector = connection.connector;
                match connector {
                    CONN_SEMI | CONN_NEWLINE => {
                        let first_is_compound = is_compound(&connection.first);
                        let first_ends_compound = is_compound(last_command(&connection.first));
                        self.command(&connection.first, indent, !first_is_compound || indent > 0);
                        if first_is_compound && indent > 0 {
                            if !self.output.ends_with(';') {
                                self.output.push(';');
                            }
                            self.output.push(' ');
                            self.command(&connection.second, 0, statement);
                            return;
                        }
                        self.newline();
                        if first_ends_compound && indent == 0 {
                            self.blankline();
                        }
                        self.command(&connection.second, indent, statement);
                    }
                    CONN_AMP => {
                        self.command(&connection.first, indent, false);
                        self.output.push_str(" &");
                        if !is_null_command(&connection.second) {
                            self.newline();
                            self.command(&connection.second, indent, statement);
                        }
                    }
                    CONN_AND_AND | CONN_OR_OR | CONN_PIPE | CONN_BAR_AND => {
                        self.command(&connection.first, indent, false);
                        let operator = match connector {
                            CONN_AND_AND => " && ",
                            CONN_OR_OR => " || ",
                            CONN_PIPE => " | ",
                            CONN_BAR_AND => " |& ",
                            _ => unreachable!(),
                        };
                        self.output.push_str(operator);
                        self.command(&connection.second, 0, false);
                        if statement {
                            self.finish_statement();
                        }
                    }
                    _ => {}
                }
                return;
            }
            CommandData::Simple(simple) => {
                self.indent(indent);
                for (index, word) in simple.words.iter().enumerate() {
                    if index > 0 {
                        self.output.push(' ');
                    }
                    self.output.push_str(&word_text(word));
                }
                for redirect in &simple.redirects {
                    self.redirect(redirect);
                }
            }
            CommandData::For(for_command) => {
                self.indent(indent);
                self.output.push_str("for ");
                self.output.push_str(&word_text(&for_command.name));
                if let Some(words) = &for_command.map_list {
                    self.output.push_str(" in");
                    for word in words {
                        self.output.push(' ');
                        self.output.push_str(&word_text(word));
                    }
                }
                self.output.push(';');
                self.newline();
                self.indent(indent);
                self.output.push_str("do");
                self.newline();
                self.block_body(&for_command.action, indent + INDENT);
                self.newline();
                self.indent(indent);
                self.output.push_str("done");
            }
            CommandData::Select(select_command) => {
                self.indent(indent);
                self.output.push_str("select ");
                self.output.push_str(&word_text(&select_command.name));
                if let Some(words) = &select_command.map_list {
                    self.output.push_str(" in");
                    for word in words {
                        self.output.push(' ');
                        self.output.push_str(&word_text(word));
                    }
                }
                self.output.push(';');
                self.newline();
                self.indent(indent);
                self.output.push_str("do");
                self.newline();
                self.block_body(&select_command.action, indent + INDENT);
                self.newline();
                self.indent(indent);
                self.output.push_str("done");
            }
            CommandData::ArithFor(arith_for) => self.arith_for(arith_for, indent),
            CommandData::Case(case_command) => self.case_command(case_command, indent),
            CommandData::While(while_command) => {
                self.indent(indent);
                self.output.push_str("while ");
                self.command(&while_command.test, 0, true);
                self.output.push_str(" do");
                self.newline();
                self.block_body(&while_command.action, indent + INDENT);
                self.newline();
                self.indent(indent);
                self.output.push_str("done");
            }
            CommandData::Until(until_command) => {
                self.indent(indent);
                self.output.push_str("until ");
                self.command(&until_command.test, 0, true);
                self.output.push_str(" do");
                self.newline();
                self.block_body(&until_command.action, indent + INDENT);
                self.newline();
                self.indent(indent);
                self.output.push_str("done");
            }
            CommandData::If(if_command) => {
                self.indent(indent);
                self.output.push_str("if ");
                self.command(&if_command.test, 0, true);
                self.output.push_str(" then");
                self.newline();
                self.block_body(&if_command.true_case, indent + INDENT);
                if let Some(false_case) = &if_command.false_case {
                    self.newline();
                    self.indent(indent);
                    self.output.push_str("else");
                    self.newline();
                    self.block_body(false_case, indent + INDENT);
                }
                self.newline();
                self.indent(indent);
                self.output.push_str("fi");
            }
            CommandData::FunctionDef(function) => {
                self.indent(indent);
                self.output.push_str(&word_text(&function.name));
                self.output.push_str(" ()");
                self.newline();
                self.command(&function.command, indent, false);
            }
            CommandData::Group(group) => {
                self.indent(indent);
                self.output.push('{');
                self.newline();
                self.block_body(&group.command, indent + INDENT);
                self.newline();
                self.indent(indent);
                self.output.push('}');
            }
            CommandData::Subshell(subshell) => {
                self.indent(indent);
                self.output.push('(');
                self.newline();
                self.block_body(&subshell.command, indent + INDENT);
                self.newline();
                self.indent(indent);
                self.output.push(')');
            }
            CommandData::Arith(arith) => {
                self.indent(indent);
                self.output.push_str("(( ");
                self.output.push_str(&word_text(&arith.expression));
                self.output.push_str(" ))");
            }
            CommandData::Cond(condition) => {
                self.indent(indent);
                self.output.push_str("[[ ");
                self.condition(condition);
                self.output.push_str(" ]]");
            }
            CommandData::Coproc(coproc) => {
                self.indent(indent);
                self.output.push_str("coproc");
                if let Some(name) = &coproc.name {
                    self.output.push(' ');
                    self.output.push_str(&word_text(name));
                }
                self.output.push(' ');
                self.command(&coproc.command, 0, false);
            }
        }

        for redirect in &command.redirects {
            self.redirect(redirect);
        }
        if statement {
            self.finish_statement();
        }
    }

    fn block_body(&mut self, command: &Command, indent: usize) {
        match &command.data {
            CommandData::Group(group) if command.redirects.is_empty() => {
                self.command(&group.command, indent, true)
            }
            _ => self.command(command, indent, true),
        }
    }

    fn arith_for(&mut self, command: &ArithForCommand, indent: usize) {
        self.indent(indent);
        self.output.push_str("for ((");
        if let Some(init) = &command.init {
            self.output.push_str(word_text(init).trim());
        }
        self.output.push_str("; ");
        if let Some(test) = &command.test {
            self.output.push_str(word_text(test).trim());
        }
        self.output.push_str("; ");
        if let Some(step) = &command.step {
            self.output.push_str(word_text(step).trim());
        }
        self.output.push_str(" ))");
        self.newline();
        self.indent(indent);
        self.output.push_str("do");
        self.newline();
        self.block_body(&command.action, indent + INDENT);
        self.newline();
        self.indent(indent);
        self.output.push_str("done");
    }

    fn case_command(&mut self, command: &CaseCommand, indent: usize) {
        self.indent(indent);
        self.output.push_str("case ");
        self.output.push_str(&word_text(&command.word));
        self.output.push_str(" in ");
        self.newline();
        for clause in &command.clauses {
            self.indent(indent + INDENT);
            for (index, pattern) in clause.patterns.iter().enumerate() {
                if index > 0 {
                    self.output.push_str(" | ");
                }
                self.output.push_str(&word_text(pattern));
            }
            self.output.push(')');
            if let Some(action) = &clause.action {
                self.newline();
                self.command(action, indent + INDENT * 2, true);
            }
            self.newline();
            self.indent(indent + INDENT * 2);
            let terminator = if clause.flags & CASEPAT_FALLTHROUGH != 0 {
                ";&"
            } else if clause.flags & CASEPAT_TESTNEXT != 0 {
                ";;&"
            } else {
                ";;"
            };
            self.output.push_str(terminator);
            self.newline();
        }
        self.indent(indent);
        self.output.push_str("esac");
    }

    fn condition(&mut self, condition: &CondCommand) {
        match condition.cond_type {
            CondType::And | CondType::Or => {
                if let Some(left) = &condition.left {
                    self.condition(left);
                }
                self.output
                    .push_str(if condition.cond_type == CondType::And {
                        " && "
                    } else {
                        " || "
                    });
                if let Some(right) = &condition.right {
                    self.condition(right);
                }
            }
            CondType::Unary => {
                if let Some(op) = &condition.op {
                    self.output.push_str(&word_text(op));
                    self.output.push(' ');
                }
                if let Some(term) = &condition.term {
                    self.output.push_str(&word_text(term));
                }
            }
            CondType::Binary => {
                if let Some(left) = &condition.left {
                    self.condition(left);
                }
                if let Some(op) = &condition.op {
                    self.output.push(' ');
                    self.output.push_str(&word_text(op));
                    self.output.push(' ');
                }
                if let Some(right) = &condition.right {
                    self.condition(right);
                }
            }
            CondType::Expr => {
                self.output.push_str("( ");
                if let Some(left) = &condition.left {
                    self.condition(left);
                }
                self.output.push_str(" )");
            }
            CondType::Term => {
                if let Some(term) = &condition.term {
                    self.output.push_str(&word_text(term));
                }
            }
        }
    }

    fn redirect(&mut self, redirect: &Redirect) {
        self.output.push(' ');
        let default_fd = match redirect.instruction {
            RedirectInstruction::InputDirection
            | RedirectInstruction::InputaDirection
            | RedirectInstruction::ReadingUntil
            | RedirectInstruction::ReadingString
            | RedirectInstruction::DeblankReadingUntil
            | RedirectInstruction::DuplicatingInput
            | RedirectInstruction::DuplicatingInputWord
            | RedirectInstruction::MoveInput
            | RedirectInstruction::MoveInputWord => 0,
            _ => 1,
        };
        match &redirect.redirector {
            Redirector::Fd(fd) if *fd != default_fd => self.output.push_str(&fd.to_string()),
            Redirector::Fd(_) => {}
            Redirector::Var(name) => {
                self.output.push('{');
                self.output.push_str(name);
                self.output.push('}');
            }
        }
        let operator =
            match redirect.instruction {
                RedirectInstruction::OutputDirection => ">",
                RedirectInstruction::InputDirection => "<",
                RedirectInstruction::InputaDirection => "<&",
                RedirectInstruction::AppendingTo => ">>",
                RedirectInstruction::ReadingUntil => "<<",
                RedirectInstruction::ReadingString => "<<<",
                RedirectInstruction::DuplicatingInput
                | RedirectInstruction::DuplicatingInputWord => "<&",
                RedirectInstruction::DuplicatingOutput
                | RedirectInstruction::DuplicatingOutputWord => ">&",
                RedirectInstruction::DeblankReadingUntil => "<<-",
                RedirectInstruction::CloseThis => match default_fd {
                    0 => "<&",
                    _ => ">&",
                },
                RedirectInstruction::ErrAndOut => "&>",
                RedirectInstruction::InputOutput => "<>",
                RedirectInstruction::OutputForce => ">|",
                RedirectInstruction::MoveInput | RedirectInstruction::MoveInputWord => "<&",
                RedirectInstruction::MoveOutput | RedirectInstruction::MoveOutputWord => ">&",
                RedirectInstruction::AppendErrAndOut => "&>>",
            };
        self.output.push_str(operator);
        match &redirect.redirectee {
            Redirectee::Fd(fd) => self.output.push_str(&fd.to_string()),
            Redirectee::Word(word) => {
                if !matches!(
                    redirect.instruction,
                    RedirectInstruction::DuplicatingInput
                        | RedirectInstruction::DuplicatingOutput
                        | RedirectInstruction::DuplicatingInputWord
                        | RedirectInstruction::DuplicatingOutputWord
                        | RedirectInstruction::MoveInput
                        | RedirectInstruction::MoveOutput
                        | RedirectInstruction::MoveInputWord
                        | RedirectInstruction::MoveOutputWord
                        | RedirectInstruction::CloseThis
                ) {
                    self.output.push(' ');
                }
                self.output.push_str(&word_text(word));
                if matches!(
                    redirect.instruction,
                    RedirectInstruction::MoveInput
                        | RedirectInstruction::MoveOutput
                        | RedirectInstruction::MoveInputWord
                        | RedirectInstruction::MoveOutputWord
                ) && !self.output.ends_with('-')
                {
                    self.output.push('-');
                }
            }
        }
        if let (Some(body), Some(delimiter)) = (&redirect.here_doc_body, &redirect.here_doc_eof) {
            self.newline();
            self.output.push_str(body);
            if !body.ends_with('\n') {
                self.newline();
            }
            self.output.push_str(delimiter);
        }
    }

    fn finish_statement(&mut self) {
        while self.output.ends_with(' ') {
            self.output.pop();
        }
        if !self.output.ends_with(';')
            && !self.output.ends_with('&')
            && !self.output.ends_with('\n')
        {
            self.output.push(';');
        }
    }

    fn indent(&mut self, width: usize) {
        if self.at_line_start() {
            self.output.extend(std::iter::repeat_n(' ', width));
        }
    }

    fn newline(&mut self) {
        while self.output.ends_with([' ', '\t']) {
            self.output.pop();
        }
        if !self.output.ends_with('\n') {
            self.output.push('\n');
        }
    }

    fn blankline(&mut self) {
        self.newline();
        if !self.output.ends_with("\n\n") {
            self.output.push('\n');
        }
    }

    fn at_line_start(&self) -> bool {
        self.output.is_empty() || self.output.ends_with('\n')
    }
}

fn word_text(word: &WordDesc) -> String {
    word.raw.clone().unwrap_or_else(|| word.text.clone())
}

fn is_null_command(command: &Command) -> bool {
    matches!(
        &command.data,
        CommandData::Simple(simple)
            if simple.words.is_empty()
                && simple.redirects.is_empty()
                && command.redirects.is_empty()
    )
}

fn is_compound(command: &Command) -> bool {
    matches!(
        command.data,
        CommandData::For(_)
            | CommandData::Case(_)
            | CommandData::While(_)
            | CommandData::Until(_)
            | CommandData::If(_)
            | CommandData::FunctionDef(_)
            | CommandData::Group(_)
            | CommandData::Select(_)
            | CommandData::ArithFor(_)
            | CommandData::Subshell(_)
            | CommandData::Coproc(_)
    )
}

fn last_command(mut command: &Command) -> &Command {
    while let CommandData::Connection(connection) = &command.data {
        if is_null_command(&connection.second) {
            command = &connection.first;
        } else {
            command = &connection.second;
        }
    }
    command
}

#[cfg(test)]
mod tests {
    use cherubsh_lexer::{Lexer, TokenKind};

    use crate::Parser;

    use super::pretty_print;

    fn render(source: &str) -> String {
        let mut lexer = Lexer::new(source);
        let mut tokens = Vec::new();
        while let Some(token) = lexer.next_token() {
            let end = token.kind == TokenKind::End;
            tokens.push(token);
            if end {
                break;
            }
        }
        let mut parser = Parser::new(tokens, source);
        let ast = parser.parse().expect("parse source");
        pretty_print(&ast)
    }

    #[test]
    fn prints_if_in_bash_style() {
        assert_eq!(
            render("if true; then echo yes; else echo no; fi\n"),
            "if true; then\n    echo yes;\nelse\n    echo no;\nfi\n\n"
        );
    }

    #[test]
    fn prints_arithmetic_for_in_bash_style() {
        assert_eq!(
            render("for ((i=1; i <= 3; i++)); do echo $i; done\n"),
            "for ((i=1; i <= 3; i++ ))\ndo\n    echo $i;\ndone\n\n"
        );
    }

    #[test]
    fn separates_top_level_compound_commands() {
        assert_eq!(
            render("for i in 1; do echo x; done\nfor j in 2; do echo y; done\n"),
            "for i in 1;\ndo\n    echo x;\ndone\n\nfor j in 2;\ndo\n    echo y;\ndone\n\n"
        );
    }
}
