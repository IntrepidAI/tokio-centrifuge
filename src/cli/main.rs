use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(disable_help_flag = true)]
#[command(disable_version_flag = true)]
#[command(version)]
struct Arguments {
    #[command(subcommand)]
    command: Option<Commands>,
    #[arg(global = true, short, long, help = "print this help message", action = clap::ArgAction::Help)]
    help: Option<bool>,
    #[arg(global = true, short = 'V', long, help = "print client version", action = clap::ArgAction::Version)]
    version: Option<bool>,
}

#[derive(Parser, Debug)]
enum Commands {
    #[command(about = "execute a project")]
    Run {
        #[arg(short, long, help = "agent id", default_value = "0")]
        agent_id: i64,
        #[arg(help = "input file or project link")]
        url: Option<String>,
        #[arg(short='S', long, help = "use simulated time", default_value = "false")]
        sim_time: bool,
        #[arg(short, long, help = "speed factor for simulated time", default_value = "1")]
        speed: f32,
    },

    #[command(about = "run a single node once with given inputs")]
    RunNode {
        #[arg(short, long, help = "agent id", default_value = "0")]
        agent_id: i64,
        #[arg(help = "node type")]
        node_type: String,
        #[arg(short, long, help = "input arguments")]
        input: Vec<String>,
        #[arg(short, long, help = "repeat the node execution with the given frequency (Hz)")]
        repeat: Option<f32>,
        #[arg(long, help = "print graph json instead of executing")]
        dump: bool,
    },

    #[command(about = "execute built-in benchmark", aliases = &["bench"])]
    Benchmark {
        #[arg(short, long, help = "number of iterations", default_value = "2000000")]
        iterations: u32,
    },

    #[command(about = "list all available types")]
    ListTypes {
        #[arg(short, long, help = "include primitive types")]
        all: bool,
    },

    #[command(about = "list all available nodes")]
    ListNodes {
        #[arg(short, long, help = "include primitive nodes")]
        all: bool,
    },

    #[command(about = "publish a single node into Intrepid hub")]
    Publish {
        #[arg(help = "node type")]
        node_type: String,
        #[arg(help = "project link")]
        url: Option<String>,
        #[arg(short, long, help = "publish even if the node already exists")]
        force: bool,
    },

    #[cfg(feature = "dbg")]
    #[command(about = "run REPL debugger")]
    Dbg {
        #[arg(help = "input file or project link")]
        url: Option<String>,
    },

    #[cfg(feature = "ir")]
    #[command(about = "generate IR of a graph from a json file")]
    Ir {
        #[arg(help = "input file, *.json")]
        path: PathBuf,
        #[arg(short, long, help = "reverse the graph", conflicts_with_all = &["check", "from_json"])]
        reverse: bool,
        #[arg(short, long, help = "check the graph", conflicts_with_all = &["reverse", "from_json"])]
        check: bool,
        #[arg(long, help = "reverse the graph from json repr", conflicts_with_all = &["check", "reverse"])]
        from_json: bool,
        #[arg(short, long, help = "graph annotation")]
        message: Option<String>,
    },

    #[cfg(feature = "expr")]
    #[command(about = "generate graph from a math expression")]
    Expr {
        #[arg(help = "math expression")]
        expr: String,
        #[arg(short, long, help = "output file, *.json")]
        output: PathBuf,
    },

    /*#[command(about = "Measure evaluation speed of a graph.")]
    Bench {
        #[arg(help = "input file, *.json")]
        path: PathBuf,
    },*/
    #[command(about = "upgrade graph to latest graph format version", hide = true)]
    Migrate {
        #[arg(help = "input file, *.json")]
        path: PathBuf,
    },

    #[command(about = "dump built-in node specs")]
    DumpMeta {
        #[arg(help = "output file, *.json")]
        path: PathBuf,
    },

    #[command(about = "print debug information", hide = true)]
    Print {
        #[arg(help = "information to print out")]
        info: String,
    },

    #[cfg(feature = "upgrade")]
    #[command(about = "upgrade executable to the latest version")]
    Upgrade {
        #[arg(help = "upgrade to a specific version (v0.0.0) or release channel (stable, dev)")]
        version: Option<String>,
        #[arg(short, long, help = "upgrade even if already up-to-date")]
        force: bool,
        #[arg(short, long, help = "dry run, do not actually upgrade")]
        dry_run: bool,
        #[arg(short, long, help = "fetch binary for a specific cpu architecture (e.g. x86_64-linux)")]
        arch: Option<String>,
    },

    #[cfg(feature = "fake-telemetry")]
    #[command(about = "telemetry test.")]
    TelemetryTest {
        #[arg(short, long, help = "agent id", default_value = "0")]
        agent_id: i64,
        #[arg(help = "project link")]
        url: String,
        #[arg(short, long, help = "speed factor", default_value = "1.0")]
        speed: f32,
    },
}

fn main() {
    let cli = Arguments::parse();

    dbg!(&cli);
}
