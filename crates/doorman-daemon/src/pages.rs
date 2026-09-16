//! Friendly HTML pages the proxy serves to browsers when it can't hand a request off.

/// Header the error pages send while polling, answered by the daemon itself.
pub const PROBE_HEADER: &str = "x-doorman-probe";

/// What an error page is waiting for before it reloads.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// A route with this name gets registered.
    Route,
    /// The route's app starts accepting connections.
    App,
    /// Nothing to wait for; the page never reloads itself.
    None,
}

impl Probe {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Route => "route",
            Self::App => "app",
            Self::None => "none",
        }
    }

    pub fn parse(value: &str) -> Self {
        if value == "route" {
            Self::Route
        } else {
            Self::App
        }
    }
}

pub struct Hint<'a> {
    pub title: &'a str,
    pub body: &'a str,
    pub command: Option<String>,
}

pub struct ErrorPage<'a> {
    pub status: u16,
    pub reason: &'a str,
    pub host: &'a str,
    pub headline: String,
    pub summary: String,
    /// Shown next to the animated pulse while the page polls for recovery.
    pub waiting_for: String,
    pub probe: Probe,
    pub hints: Vec<Hint<'a>>,
    pub detail: Option<String>,
}

impl ErrorPage<'_> {
    pub fn render(&self) -> String {
        let hints = self
            .hints
            .iter()
            .enumerate()
            .map(|(index, hint)| {
                let command = hint.command.as_deref().map_or_else(String::new, |command| {
                    format!(
                        r#"<div class="cmd"><code>{}</code><button type="button" data-copy="{}">Copy</button></div>"#,
                        escape(command),
                        escape(command)
                    )
                });
                format!(
                    r#"<li style="--i:{index}"><span class="step">{}</span><div><h3>{}</h3><p>{}</p>{command}</div></li>"#,
                    index + 1,
                    escape(hint.title),
                    hint.body
                )
            })
            .collect::<String>();
        let detail = self.detail.as_deref().map_or_else(String::new, |detail| {
            format!(
                "<details><summary>Technical details</summary><pre>{}</pre></details>",
                escape(detail)
            )
        });

        TEMPLATE
            .replace("{{status}}", &self.status.to_string())
            .replace("{{reason}}", &escape(self.reason))
            .replace("{{host}}", &escape(self.host))
            .replace("{{headline}}", &self.headline)
            .replace("{{summary}}", &self.summary)
            .replace("{{waiting}}", &self.waiting_for)
            .replace("{{probe}}", self.probe.as_str())
            .replace("{{hints}}", &hints)
            .replace("{{detail}}", &detail)
    }
}

pub fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

const TEMPLATE: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{{status}} · {{host}} · Doorman</title>
<style>
  :root {
    --bg: #0a0a0a; --surface: #111; --border: #232323; --text: #ededed;
    --muted: #a1a1a1; --faint: #6b6b6b; --accent: #8e7cf8; --amber: #f5a524;
    color-scheme: dark;
  }
  * { box-sizing: border-box; }
  body {
    margin: 0; min-height: 100vh; display: grid; place-items: center;
    padding: 48px 20px; background: var(--bg); color: var(--text);
    font: 14px/1.55 -apple-system, BlinkMacSystemFont, "Inter", "Segoe UI", sans-serif;
    background-image: radial-gradient(600px 300px at 50% -80px, rgba(142,124,248,.12), transparent);
  }
  main { width: 100%; max-width: 560px; }
  code, pre { font-family: ui-monospace, "SF Mono", Menlo, monospace; }

  .scene { position: relative; width: 72px; height: 80px; margin: 0 auto 24px; }
  .door {
    position: absolute; left: 50%; bottom: 8px; width: 46px; height: 62px;
    margin-left: -23px; border-radius: 7px 7px 2px 2px;
    background: linear-gradient(180deg, #1c1c1c, #141414);
    border: 1px solid #333; box-shadow: 0 10px 30px rgba(0,0,0,.6);
    transform-origin: 50% 100%; animation: knock 2.4s ease-in-out infinite;
  }
  .door::before {
    content: ""; position: absolute; inset: 8px 8px 20px; border-radius: 3px;
    border: 1px solid #2a2a2a;
  }
  .door::after {
    content: ""; position: absolute; right: 8px; top: 34px; width: 5px; height: 5px;
    border-radius: 50%; background: var(--amber); box-shadow: 0 0 10px var(--amber);
  }
  .floor {
    position: absolute; left: 0; right: 0; bottom: 8px; height: 1px;
    background: linear-gradient(90deg, transparent, #333, transparent);
  }
  @keyframes knock {
    0%, 60%, 100% { transform: translateX(0) rotate(0); }
    64% { transform: translateX(-2px) rotate(-1.5deg); }
    68% { transform: translateX(2px) rotate(1.5deg); }
    72% { transform: translateX(-1px) rotate(-.8deg); }
    76% { transform: translateX(0) rotate(0); }
  }

  .eyebrow {
    display: flex; justify-content: center; gap: 8px; margin: 0 0 10px;
    color: var(--faint); font-size: 12px; letter-spacing: .02em;
  }
  .eyebrow b { color: var(--amber); font-weight: 600; }
  h1 { margin: 0; text-align: center; font-size: 26px; line-height: 1.25; font-weight: 600; letter-spacing: -.02em; }
  h1 code { font-size: .88em; font-weight: 500; color: #fff; overflow-wrap: anywhere; }
  .summary { margin: 10px auto 0; max-width: 480px; text-align: center; color: var(--muted); }
  .summary code { color: var(--text); overflow-wrap: anywhere; }

  .status {
    display: flex; align-items: center; justify-content: center; gap: 10px;
    margin: 22px auto 30px; width: fit-content; max-width: 100%;
    padding: 6px 14px 6px 10px; border: 1px solid var(--border); border-radius: 999px;
    color: var(--muted); font-size: 12px;
  }
  .pulse { position: relative; width: 8px; height: 8px; flex: none; }
  .pulse::before, .pulse::after {
    content: ""; position: absolute; inset: 0; border-radius: 50%; background: var(--amber);
  }
  .pulse::after { animation: pulse 1.6s ease-out infinite; }
  @keyframes pulse { to { transform: scale(2.8); opacity: 0; } }
  .status.up { color: #3ecf8e; border-color: #1d4d38; }
  .status.up .pulse::before, .status.up .pulse::after { background: #3ecf8e; }

  ol { list-style: none; margin: 0; padding: 0; border: 1px solid var(--border); border-radius: 10px; background: var(--surface); overflow: hidden; }
  li {
    display: grid; grid-template-columns: 22px minmax(0, 1fr); gap: 14px; padding: 16px 18px;
    opacity: 0; transform: translateY(6px);
    animation: rise .5s cubic-bezier(.2,.7,.3,1) forwards; animation-delay: calc(.15s + var(--i) * .08s);
  }
  li + li { border-top: 1px solid var(--border); }
  @keyframes rise { to { opacity: 1; transform: none; } }
  .step {
    width: 22px; height: 22px; border-radius: 50%; display: grid; place-items: center;
    border: 1px solid #333; color: var(--muted); font-size: 11px; font-weight: 600;
  }
  h3 { margin: 1px 0 2px; font-size: 13px; font-weight: 600; }
  li p { margin: 0; color: var(--muted); font-size: 13px; }
  li p code { color: var(--text); font-size: 12px; overflow-wrap: anywhere; }
  .cmd {
    display: flex; align-items: center; gap: 8px; margin-top: 10px;
    padding: 4px 4px 4px 12px; border: 1px solid var(--border); border-radius: 6px; background: var(--bg);
  }
  .cmd code {
    flex: 1; min-width: 0; overflow-x: auto; white-space: nowrap; font-size: 12px; padding: 5px 0;
    scrollbar-width: none;
  }
  .cmd code::-webkit-scrollbar { display: none; }
  .cmd code::before { content: "$ "; color: var(--faint); }
  button {
    flex: none; font: inherit; font-size: 12px; color: var(--text); cursor: pointer;
    background: #1a1a1a; border: 1px solid #2c2c2c; border-radius: 5px; padding: 4px 10px;
  }
  button:hover { background: #222; }

  details { margin-top: 18px; color: var(--faint); font-size: 12px; }
  summary { cursor: pointer; width: fit-content; }
  pre { margin: 8px 0 0; padding: 12px; white-space: pre-wrap; word-break: break-word; border: 1px solid var(--border); border-radius: 6px; background: var(--surface); color: var(--muted); }
  .status[data-probe="none"] { display: none; }
  footer { margin-top: 28px; text-align: center; color: var(--faint); font-size: 12px; }

  @media (prefers-reduced-motion: reduce) {
    *, *::before, *::after { animation: none !important; }
    li { opacity: 1; transform: none; }
  }
</style>
</head>
<body>
<main>
  <div class="scene" aria-hidden="true">
    <span class="floor"></span><span class="door"></span>
  </div>
  <p class="eyebrow"><b>{{status}}</b><span>{{reason}}</span></p>
  <h1>{{headline}}</h1>
  <p class="summary">{{summary}}</p>
  <div class="status" id="status" role="status" data-probe="{{probe}}"><span class="pulse"></span><span id="status-text">{{waiting}}</span></div>
  <ol>{{hints}}</ol>
  {{detail}}
  <footer>Served by Doorman · this page reloads by itself once things are fixed</footer>
</main>
<script>
  document.addEventListener("click", async (event) => {
    const button = event.target.closest("[data-copy]");
    if (!button) return;
    try {
      await navigator.clipboard.writeText(button.dataset.copy);
      button.textContent = "Copied";
      setTimeout(() => (button.textContent = "Copy"), 1400);
    } catch {}
  });

  // Ask Doorman (not the app) whether the route is reachable yet; reload when it is.
  const poll = async () => {
    try {
      const response = await fetch(location.href, {
        method: "HEAD", cache: "no-store", headers: { "X-Doorman-Probe": "{{probe}}" },
      });
      if (response.status === 204) {
        const status = document.getElementById("status");
        status.classList.add("up");
        document.getElementById("status-text").textContent = "It's up. Reloading…";
        setTimeout(() => location.reload(), 350);
        return;
      }
    } catch {}
    setTimeout(poll, 1500);
  };
  if ("{{probe}}" !== "none") setTimeout(poll, 1500);
</script>
</body>
</html>
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_interpolated_values() {
        let page = ErrorPage {
            status: 502,
            reason: "Bad gateway",
            host: "<script>",
            headline: String::new(),
            summary: String::new(),
            waiting_for: String::new(),
            probe: Probe::App,
            hints: vec![Hint {
                title: "t",
                body: "b",
                command: Some("echo \"<x>\"".into()),
            }],
            detail: Some("<b>".into()),
        }
        .render();
        assert!(!page.contains("· <script> ·"));
        assert!(page.contains("&lt;script&gt;"));
        assert!(page.contains("echo &quot;&lt;x&gt;&quot;"));
        assert!(page.contains("<pre>&lt;b&gt;</pre>"));
    }
}
