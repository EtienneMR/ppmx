use anyhow::Result;

mod backend;
mod frontend;

fn main() -> Result<()> {
    frontend::run()
}
