//! 首启缺配置时的提示页：issue 036「首启无配置 → 内嵌一页可读提示（写清把
//! providers.toml 放哪）——不 panic」。这个文件只干一件事——把一条诊断消息
//! 变成一个可以让主窗口导航过去的 `file://` 页面,不掺「怎么判断缺配置」（那是
//! `server` 模块的事,它拿 `agent_server::BootstrapError` 判)。
//!
//! # 为什么是 `file://` 页面，不是复用 `AgentServer::with_static_dir`
//!
//! 复用静态托管需要先有一个能塞进 `ServerConfig::new` 的 `SessionTemplate`——
//! 但走到这个模块，恰恰是因为 provider/key 还没配好，凑不出一份真实的
//! `SessionTemplate`。伪造一份「反正没人会用」的假模板去换一次
//! `with_static_dir` 复用,是在制造一个「为什么这个模板的字段全是假的」的
//! 疑问,不比直接写文件 + `file://` 导航更省心;这条路径也从不经过
//! loopback/HTTP,红线 8 不适用。

use tauri::Url;

/// 写一页说明到 `config_dir/first-run.html`，返回可以直接喂给
/// `WebviewWindow::navigate` 的 `file://` URL。`config_dir` 本身可能还不存在
/// （用户从没跑过这个应用）——现造。
pub fn write_page(config_dir: &std::path::Path, providers_toml_path: &std::path::Path, reason: &str) -> std::io::Result<Url> {
    std::fs::create_dir_all(config_dir)?;
    let page = config_dir.join("first-run.html");
    std::fs::write(&page, render(providers_toml_path, reason))?;
    Url::from_file_path(&page).map_err(|_| std::io::Error::other(format!("造不出 file:// URL：{}", page.display())))
}

fn render(providers_toml_path: &std::path::Path, reason: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="zh">
<head>
<meta charset="UTF-8" />
<title>agent-desktop：还没配置</title>
<style>
  body {{ font-family: -apple-system, sans-serif; max-width: 640px; margin: 3rem auto; line-height: 1.6; color: #222; padding: 0 1.5rem; }}
  code, pre {{ background: #f2f2f2; padding: 0.15rem 0.4rem; border-radius: 4px; }}
  pre {{ padding: 1rem; overflow-x: auto; }}
  h1 {{ font-size: 1.3rem; }}
</style>
</head>
<body>
<h1>还没找到可用的 provider 配置</h1>
<p>{reason}</p>
<p>把 provider 的连接信息（provider 名字、endpoint、model、key）写进这个文件，
然后重新打开本应用：</p>
<pre>{path}</pre>
<p>文件形状参考仓库里的 <code>providers.example.toml</code>，最小示例：</p>
<pre>[providers.deepseek]
api_key_env = "DEEPSEEK_API_KEY"
base_url = "https://api.deepseek.com"
model = "deepseek-v4-pro"

[default]
provider = "deepseek"</pre>
<p>key 也可以直接写 <code>api_key = "..."</code>（不建议长期这么放，方便临时试）。</p>
</body>
</html>
"#,
        reason = html_escape(reason),
        path = html_escape(&providers_toml_path.display().to_string()),
    )
}

/// 提示页里唯一会插入用户/环境相关文本的地方是错误信息和路径——两者都不会
/// 包含用户可控输入（本地磁盘路径、库自己拼的错误文案），转义只是防御性的,
/// 不值得为此拉一个 html 转义依赖。
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
