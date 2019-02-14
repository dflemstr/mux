use std::path;

#[derive(Debug, StructOpt)]
#[structopt(name = "mux")]
pub struct Options {
    /// Items are separated by a null, not whitespace; disables quote and backslash processing and
    /// logical EOF processing.
    #[structopt(short = "0", long = "null")]
    pub null: bool,

    /// Read arguments from FILE, not standard input.
    #[structopt(short = "a", long = "arg-file", value_name = "FILE")]
    pub arg_file: Option<path::PathBuf>,

    /// Items in input stream are separated by SEP, not by whitespace; disables quote and
    /// backslash processing and logical EOF processing.
    #[structopt(
        short = "d",
        long = "delimiter",
        value_name = "SEP",
        parse(try_from_str = "parse_delimiter")
    )]
    pub delimiter: Option<u8>,

    /// Set logical EOF string; if END occurs as a line of input, the rest of the input is ignored
    /// (ignored if -0 or -d was specified).
    #[structopt(short = "e", long = "eof", value_name = "END", visible_alias = "E")]
    pub end: Option<String>,

    /// Replace R in INITIAL-ARGS with names read from standard input; if R is unspecified, assume
    /// {}.
    #[structopt(short = "i", long = "replace", value_name = "R", visible_alias = "I")]
    pub replace: Option<String>,

    /// Use at most MAX-LINES non-blank input lines per command line.
    #[structopt(
        short = "L",
        long = "max-lines",
        value_name = "MAX-LINES",
        visible_alias = "l"
    )]
    pub max_lines: Option<u64>,

    /// Use at most MAX-ARGS arguments per command line.
    #[structopt(short = "n", long = "max-args", value_name = "MAX-ARGS")]
    pub max_args: Option<u64>,

    /// Run at most MAX-PROCS processes at a time.
    #[structopt(short = "P", long = "max-procs", value_name = "MAX-PROCS")]
    pub max_procs: Option<u64>,

    /// Prompt before running commands.
    #[structopt(short = "p", long = "interactive")]
    pub interactive: bool,

    /// Set environment variable VAR in child processes.
    #[structopt(long = "process-slot-var", value_name = "VAR")]
    pub process_slot_var: Vec<String>,

    /// If there are no arguments, then do not run COMMAND; if this option is not given, COMMAND
    /// will be run at least once.
    #[structopt(short = "r", long = "no-run-if-empty")]
    pub no_run_if_empty: bool,

    /// Limit length of command line to MAX-CHARS.
    #[structopt(short = "s", long = "max-chars", value_name = "MAX-CHARS")]
    pub max_chars: Option<u64>,

    /// Show limits on command-line length.
    #[structopt(long = "show-limits")]
    pub show_limits: bool,

    /// Print commands before executing them.
    #[structopt(short = "t", long = "verbose")]
    pub verbose: bool,

    /// Exit if the size (see -s) is exceeded.
    #[structopt(short = "x", long = "exit")]
    pub exit: bool,

    #[structopt(value_name = "COMMAND")]
    pub command: String,

    #[structopt(value_name = "INITIAL-ARGS")]
    pub initial_args: Vec<String>,
}

fn parse_delimiter(delimiter: &str) -> Result<u8, failure::Error> {
    // TODO: add xargs features such as escape sequence parsing, octal etc
    if delimiter.len() == 1 {
        Ok(delimiter.as_bytes()[0])
    } else {
        Err(failure::err_msg(format!(
            "not a single ASCII character: {:?}",
            delimiter
        )))
    }
}
