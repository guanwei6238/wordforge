import { useCallback, useEffect, useState } from "react";
import {
  type CliAvailability,
  type CliPreset,
  detectAiBackends,
  errorMessage,
  getLlmSettings,
  type LlmSettings,
  testLlm,
  updateLlmSettings,
} from "../api";

/** 每個 CLI 怎麼呼叫。這些都是實測過的參數。 */
const CLI_SPECS: Record<
  string,
  { args: string[]; systemFlag: string | null; modelFlag: string | null; model: string; models: string[] }
> = {
  claude_code: {
    args: ["-p", "--output-format", "text"],
    systemFlag: "--append-system-prompt",
    modelFlag: "--model",
    model: "sonnet",
    models: ["haiku", "sonnet", "opus"],
  },
  codex: {
    // 資料目錄不是 git repo，不加這個會直接拒絕執行
    args: ["exec", "--skip-git-repo-check"],
    systemFlag: null,
    modelFlag: "-m",
    model: "",
    models: [],
  },
};

/**
 * 目前的設定對應到下拉選單的哪一個值。
 *
 * CLI 與 API 攤平在同一層，所以要把「後端種類 + 細節」壓成一個字串。
 */
function choiceId(settings: LlmSettings): string {
  if (settings.backend === "cli") return `cli:${settings.cli.preset}`;
  if (settings.backend === "api") {
    if (settings.api.provider === "anthropic") return "api:anthropic";
    if (settings.api.provider === "ollama") return "api:ollama";
    return "api:openai";
  }
  return "none";
}

/**
 * AI 後端設定。
 *
 * 選項刻意攤平成一層：原本要先選「本機 CLI」才會冒出 claude / codex，
 * 使用者根本不知道那裡有東西。順便直接偵測機器上裝了什麼，
 * 沒裝的標示出來而不是讓人選了才失敗。
 */
export default function LlmSetup({ onChanged }: { onChanged?: () => void }) {
  const [settings, setSettings] = useState<LlmSettings | null>(null);
  const [detected, setDetected] = useState<CliAvailability[]>([]);
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const [s, d] = await Promise.all([getLlmSettings(), detectAiBackends()]);
      setSettings(s);
      setDetected(d);
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

  const current = settings;
  const spec = CLI_SPECS[current.cli.preset];

  function choose(id: string) {
    if (id === "none") {
      void save({ ...current, backend: "none" });
      return;
    }

    if (id.startsWith("cli:")) {
      const preset = id.slice(4) as CliPreset;
      const s = CLI_SPECS[preset];
      const found = detected.find((d) => d.preset === preset);
      void save({
        ...current,
        backend: "cli",
        cli: {
          ...current.cli,
          preset,
          program: found?.program ?? preset,
          args: s?.args ?? [],
          system_flag: s?.systemFlag ?? null,
          model_flag: s?.modelFlag ?? null,
          model: s?.model ?? "",
        },
      });
      return;
    }

    const provider =
      id === "api:anthropic"
        ? ("anthropic" as const)
        : id === "api:ollama"
          ? ("ollama" as const)
          : ("open_ai_compatible" as const);
    void save({
      ...current,
      backend: "api",
      api: {
        ...current.api,
        provider,
        model:
          provider === "anthropic"
            ? "claude-sonnet-5"
            : provider === "ollama"
              ? "qwen3:8b"
              : "gpt-5",
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
        <select value={choiceId(current)} onChange={(e) => choose(e.target.value)}>
          <option value="none">不使用（只背單字）</option>
          <optgroup label="本機 CLI — 用你已經付的訂閱">
            {detected.map((d) => (
              <option key={d.preset} value={`cli:${d.preset}`} disabled={!d.installed}>
                {d.label}
                {d.installed ? `　✓ ${d.version ?? "已安裝"}` : "　（未安裝）"}
              </option>
            ))}
          </optgroup>
          <optgroup label="API">
            <option value="api:anthropic">Anthropic API 金鑰</option>
            <option value="api:openai">OpenAI 相容端點</option>
            <option value="api:ollama">Ollama（本機，完全離線）</option>
          </optgroup>
        </select>
      </label>

      {current.backend === "cli" && (
        <>
          <p className="muted hint">
            執行 <code>{[current.cli.program, ...current.cli.args].join(" ")}</code>
            ，prompt 從 stdin 送入。
          </p>

          <label>
            模型
            {spec && spec.models.length > 0 ? (
              <select
                value={current.cli.model}
                onChange={(e) => save({ ...current, cli: { ...current.cli, model: e.target.value } })}
              >
                <option value="">（CLI 預設）</option>
                {spec.models.map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </select>
            ) : (
              <input
                value={current.cli.model}
                placeholder="（CLI 預設）"
                onChange={(e) => save({ ...current, cli: { ...current.cli, model: e.target.value } })}
              />
            )}
          </label>
          <p className="muted hint">
            出題與批改是「照著明確規格產生結構化輸出」，中等模型就夠用，
            而且快得多、也比較不會撞到訂閱的速率限制。
          </p>
          <p className="muted hint">
            比 API 慢（每題要啟動一個行程，可能幾十秒），訂閱也有速率限制，
            連續出很多題會撞到。
          </p>
        </>
      )}

      {current.backend === "api" && (
        <>
          <label>
            模型
            <input
              value={current.api.model}
              onChange={(e) => save({ ...current, api: { ...current.api, model: e.target.value } })}
            />
          </label>

          {current.api.provider === "open_ai_compatible" && (
            <label>
              端點
              <input
                value={current.api.base_url ?? ""}
                placeholder="https://api.openai.com/v1"
                onChange={(e) =>
                  save({
                    ...current,
                    api: { ...current.api, base_url: e.target.value || null },
                  })
                }
              />
            </label>
          )}

          {current.api.provider !== "ollama" && (
            <>
              <label>
                API 金鑰
                <input
                  type="password"
                  value={current.api.api_key}
                  placeholder={current.api.has_api_key ? "（已設定，留空不變更）" : "sk-…"}
                  onChange={(e) =>
                    save({ ...current, api: { ...current.api, api_key: e.target.value } })
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

      {current.backend !== "none" && (
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
