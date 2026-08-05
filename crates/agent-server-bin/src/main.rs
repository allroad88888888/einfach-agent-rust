//! `agent-server` 的默认宿主二进制（issue 035）：ROADMAP 决策 12「库是主体，
//! bin 只是众多宿主之一」的另一半——企业不用它也能直接内嵌 `crates/
//! agent-server` 这个库，用它就是开箱即跑。这个文件只做「解析参数、调库」，
//! 目标二十行量级（issue 035 原文）：读 providers.toml、装配 `SessionTemplate`
//! 全部收编进 [`agent_server::bootstrap`]（跟 `examples/serve.rs` 共用），
//! `--sessions-dir`/`--port`/Ctrl-C 优雅退出这几件这个 bin 独有的事在
//! [`run`] 模块，参数解析在 [`cli`] 模块——三十行以上的装配逻辑不堆在这里。

mod cli;
mod ready_file;
mod remote_tool_timeout;
mod run;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    match cli::parse(&args) {
        cli::ParsedArgs::Help => println!("{}", cli::HELP),
        cli::ParsedArgs::Invalid(message) => {
            eprintln!("{message}\n\n{}", cli::HELP);
            std::process::exit(2);
        }
        cli::ParsedArgs::Run(opts) => run::run(opts).await,
    }
}
