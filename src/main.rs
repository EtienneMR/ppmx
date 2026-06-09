use anyhow::Result;

mod backend;
mod cli;

fn main() -> Result<()> {
    cli::run()
}
