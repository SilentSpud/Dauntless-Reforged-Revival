//! A Windows command line from a program and its arguments, as the MSVC runtime (and Unreal's own
//! parser after it) splits it back: an argument with a space or a quote is quoted, quotes and the
//! backslashes before them escaped. The game server's map and Behemoth come from the client's travel
//! URL through the backend, so an argument must never split into two (docs/security/director.md).

/// One argument as the MSVC runtime splits a command line.
pub fn quote(arg: &str, out: &mut String) {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\u{b}', '"']) {
        out.push_str(arg);
        return;
    }
    out.push('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
}

/// The program (quoted whole, as the first token is read) and its arguments.
pub fn command_line(program: &str, args: &[String]) -> String {
    let mut line = format!("\"{program}\"");
    for arg in args {
        line.push(' ');
        quote(arg, &mut line);
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_arguments_as_the_msvc_runtime_reads_them() {
        let args = [
            "plain".to_owned(),
            r#"-LogCmds="A B""#.to_owned(),
            String::new(),
            r"a\b\".to_owned(),
            r"x y\".to_owned(),
        ];
        assert_eq!(
            command_line(r"C:\Game\x.exe", &args),
            r#""C:\Game\x.exe" plain "-LogCmds=\"A B\"" "" a\b\ "x y\\""#
        );
    }
}
