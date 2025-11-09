use clap::{Parser, Subcommand, Args};

#[derive(Debug, Parser)]
#[command(name = "lab", version, about = "Research Lab CLI")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Initialize data directories (future)
    Init,
    /// Run the agent flow once
    Run(RunCmd),
    /// Chat: send a message and run the full pipeline, printing agent responses
    Chat(ChatCmd),
    /// Start HTTP server
    Serve(ServeCmd),
}

#[derive(Debug, Args)]
pub struct RunCmd {
    /// Optional goal title to seed the PI node
    #[arg(long)]
    pub goal: Option<String>,
}

#[derive(Debug, Args)]
pub struct ChatCmd {
    /// Initial message to seed as the goal; if omitted, you will be prompted
    pub message: Option<String>,
}

#[derive(Debug, Args)]
pub struct ServeCmd {
    /// Port to bind the HTTP server
    #[arg(long, default_value_t = 8080)]
    pub port: u16,
}
