import { useCallback, useEffect, useState } from "react";
import {
  type Backend,
  type CliPreset,
  errorMessage,
  getLlmSettings,
  type LlmSettings,
  testLlm,
  updateLlmSettings,
} from "../api";

const CLI_PRESETS: { value: CliPreset; label: string; program: string; args: string[]; systemFlag: string | null; hint: string }[] = [
  {
    value: "claude_code",
    label: "Claude Code",
    program: "claude",
    args: ["-p", "--output-format", "text"],
    systemFlag: "--append-system-prompt",
    hint: "用你現有的 Claude 訂閱，不必另開 API 帳單",
  },
  {
    value: "codex",
    label: "OpenAI Codex",
    program: "codex",
    args: ["exec", "--skip-git-repo-check"],
    systemFlag: null,
    hint: "用你現有的 ChatGPT 訂閱",
  },
];

/**
 * AI 後端設定。
 *
 * 三條路：用本機已登入的 CLI（訂閱）、填 API key、或接本機 Ollama。
 * 第一條通常最划算——已經付過的錢不該再付一次。
 */
export default function LlmSetup({ onChanged }: { onChanged?: () => void }) {
  const [settings, setSettings] = useState<LlmSettings | null>(null);
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setSettings(await getLlmSettings());
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  async function save(next: LlmSettings) {
    setSettings(next);
    setError(null);
    setResult(null);
    try {
      setSettings(await updateLlmSettings(next));
      onChanged?.();
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  async function runTest() {
    setTesting(true);
    setResult(null);
    setError(null);
    try {
      setResult(await testLlm());
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setTesting(false);
    }
  }

  if (!settings) return <p className="muted">載入中…</p>;

  function chooseBackend(backend: Backend) {
    void save({ ...settings!, backend });
  }

  function choosePreset(preset: CliPreset) {
    const p = CLI_PRESETS.find((c) => c.value === preset);
    if (!p) return;
    void save({
      ...settings!,
      backend: "cli",
      cli: {
        ...settings!.cli,
        preset,
        program: p.program,
        args: p.args,
        system_flag: p.systemFlag,
        model: p.label,
      },
    });
  }

  return (
    <section className="panel">
      <h2>AI 後端</h2>
      <p className="muted">
        出題與批改需要模型。背單字與查字典不需要，沒設定也能用。
      </p>

      <label>
        使用
        <select value={settings.backend} onChange={(e) => chooseBackend(e.target.value as Backend)}>
          <option value="none">不使用（只背單字）</option>
          <option value="cli">本機 CLI（用既有訂閱）</option>
          <option value="api">API 金鑰 / Ollama</option>
        </select>
      </label>

      {settings.backend === "cli" && (
        <>
          <label>
            指令
            <select
              value={settings.cli.preset}
              onChange={(e) => choosePreset(e.target.value as CliPreset)}
            >
              {CLI_PRESETS.map((p) => (
                <option key={p.value} value={p.value}>
                  {p.label}
                </option>
              ))}
              <option value="custom">自訂</option>
            </select>
          </label>

          {settings.cli.preset === "custom" ? (
            <label>
              執行檔
              <input
                value={settings.cli.program}
                onChange={(e) =>
                  save({ ...settings, cli: { ...settings.cli, program: e.target.value } })
                }
                placeholder="claude"
              />
            </label>
          ) : (
            <p className="muted hint">
              {CLI_PRESETS.find((p) => p.value === settings.cli.preset)?.hint}
              。執行 <code>{[settings.cli.program, ...settings.cli.args].join(" ")}</code>
              ，prompt 從 stdin 送入。
            </p>
          )}

          <p className="muted hint">
            比 API 慢（每題要啟動一個行程，可能幾十秒），而且訂閱有速率限制，
            連續出很多題會撞到。
          </p>
        </>
      )}

      {settings.backend === "api" && (
        <>
          <label>
            供應商
            <select
              value={settings.api.provider}
              onChange={(e) =>
                save({
                  ...settings,
                  api: { ...settings.api, provider: e.target.value as never },
                })
              }
            >
              <option value="anthropic">Anthropic</option>
              <option value="open_ai_compatible">OpenAI 相容</option>
              <option value="ollama">Ollama（本機）</option>
            </select>
          </label>

          <label>
            模型
            <input
              value={settings.api.model}
              onChange={(e) => save({ ...settings, api: { ...settings.api, model: e.target.value } })}
            />
          </label>

          {settings.api.provider !== "ollama" && (
            <>
              <label>
                API 金鑰
                <input
                  type="password"
                  value={settings.api.api_key}
                  placeholder={settings.api.has_api_key ? "（已設定，留空不變更）" : "sk-…"}
                  onChange={(e) =>
                    save({ ...settings, api: { ...settings.api, api_key: e.target.value } })
                  }
                />
              </label>
              <p className="muted hint">
                金鑰存在資料目錄的獨立檔案（權限 600），不會寫進資料庫——
                資料庫常被複製到雲端硬碟同步。
              </p>
            </>
          )}
        </>
      )}

      {settings.backend !== "none" && (
        <div className="row">
          <button onClick={runTest} disabled={testing}>
            {testing ? "測試中…" : "測試連線"}
          </button>
        </div>
      )}

      {result && <p className="ok">{result}</p>}
      {error && <p className="error">{error}</p>}
    </section>
  );
}
