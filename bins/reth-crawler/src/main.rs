mod crawler;
mod p2p;
use clap::{Args, Parser, Subcommand};
use crawler::CrawlerFactory;

#[derive(Parser)]
#[command(author, version)]
#[command(
    about = "Reth crawler",
    long_about = "Reth crawler is a standalone program that crawls the p2p network.

One can use this crawler to quantify how many Ethereum nodes exists and what is the distribution of clients."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start crawling the network
    Crawl(CrawlOpts),
}

#[derive(Args)]
struct CrawlOpts {
    #[arg(long)]
    /// Use a sqlite db for local testing.
    local_db: bool,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match &cli.command {
        Commands::Crawl(opts) => {
            let (_, _, _) = CrawlerFactory::new()
                .await
                .make(opts.local_db)
                .await
                .run()
                .await;
        }
    }
}
