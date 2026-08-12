import { useCallback, useEffect, useState } from "react";
import {
  type CliAvailability,
  type CliPreset,
  type EffortStyle,
  detectAiBackends,
  errorMessage,
  getLlmSettings,
  type LlmSettings,
  type ModelProbe,
  probeModel,
  testLlm,
  updateLlmSettings,
} from "../api";

/**
 * 每個 CLI 怎麼呼叫。這些都是實測過的參數。
 *
 * 模型與推理強度的**清單**不在這裡——那份由後端提供（`CliAvailability.options`），
 * 免得同一份清單在前後端各維護一次然後長歪。
 */
const CLI_SPECS: Record<
  string,
  {
    args: string[];
    systemFlag: string | null;
    modelFlag: string | null;
    model: string;
    effortStyle: EffortStyle;
    effort: string;
  }
> = {
  claude_code: {
    args: ["-p", "--output-format", "text"],
    systemFlag: "--append-system-prompt",
    modelFlag: "--model",
    model: "sonnet",
    effortStyle: { kind: "flag", value: "--effort" },
    effort: "medium",
  },
  codex: {
    // 資料目錄不是 git repo，不加這個會直接拒絕執行
    args: ["exec", "--skip-git-repo-check"],
    systemFlag: null,
    modelFlag: "-m",
    // 需要 codex-cli 0.147 以上；舊版會回 "requires a newer version"，
    // 設定頁的「試跑」會直接說要跑 codex update
    model: "gpt-5.6-luna",
    // codex 沒有獨立的 effort 旗標
    effortStyle: { kind: "config", value: { flag: "-c", key: "model_reasoning_effort" } },
    effort: "medium",
  },
};

/** 下拉選單裡代表「我要自己打」的值。用一個不會跟模型名撞到的字串。 */
const CUSTOM = "\u0000custom";

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
 * 「從清單挑，或自己打」的欄位。
 *
 * 純文字輸入框使用者不知道該填什麼；純下拉選單則會在清單過期時
 * 把人鎖死。兩個都要有。
 */
function Choice({
  label,
  value,
  options,
  emptyLabel,
  onChange,
}: {
  label: string;
  value: string;
  options: string[];
  emptyLabel: string;
  onChange: (value: string) => void;
}) {
  // 目前的值不在清單裡，就代表使用者自訂過，要維持在自訂模式
  const [custom, setCustom] = useState(value !== "" && !options.includes(value));

  if (options.length === 0 || custom) {
    return (
      <label>
        {label}
        <input value={value} placeholder={emptyLabel} onChange={(e) => onChange(e.target.value)} />
        {options.length > 0 && (
          <button className="link" onClick={() => setCustom(false)}>
            回到清單
          </button>
        )}
      </label>
    );
  }

  return (
    <label>
      {label}
      <select
        value={value}
        onChange={(e) => {
          if (e.target.value === CUSTOM) {
            setCustom(true);
            return;
          }
          onChange(e.target.value);
        }}
      >
        <option value="">{emptyLabel}</option>
        {options.map((o) => (
          <option key={o} value={o}>
            {o}
          </option>
        ))}
        <option value={CUSTOM}>自訂…</option>
      </select>
    </label>
  );
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
  const [probing, setProbing] = useState(false);
  const [probe, setProbe] = useState<ModelProbe | null>(null);
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

  async function runProbe() {
    setProbing(true);
    setProbe(null);
    try {
      setProbe(await probeModel(settings?.cli.model ?? ""));
    } catch (e) {
      setProbe({ usable: false, detail: errorMessage(e) });
    } finally {
      setProbing(false);
    }
  }

  if (!settings) return <p className="muted">載入中…</p>;

  const current = settings;
  // 清單來自後端；偵測不到就退回空的，欄位變成純文字輸入
  const options = detected.find((d) => d.preset === current.cli.preset)?.options;
  const models = options?.models ?? [];
  const efforts = options?.efforts ?? [];

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
          effort_style: s?.effortStyle ?? { kind: "unsupported" },
          effort: s?.effort ?? "",
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

          <Choice
            label="模型"
            value={current.cli.model}
            options={models}
            emptyLabel="（CLI 預設）"
            onChange={(model) => {
              setProbe(null);
              void save({ ...current, cli: { ...current.cli, model } });
            }}
          />
          <div className="row">
            <button onClick={runProbe} disabled={probing}>
              {probing ? "試跑中…" : "試跑這個模型"}
            </button>
            {probe && (
              <span className={probe.usable ? "ok" : "error"}>
                {probe.usable ? "可以用" : "不能用"}
              </span>
            )}
          </div>
          {probe && !probe.usable && <pre className="probe-error">{probe.detail}</pre>}
          <p className="muted hint">
            出題與批改是「照著明確規格產生結構化輸出」，中等模型就夠用，
            而且快得多、也比較不會撞到訂閱的速率限制。
          </p>
          <p className="muted hint">
            清單是已知可用的選項，<strong>一定會過期</strong>——兩個 CLI
            都沒有可以查詢模型清單的指令，所以沒辦法自動更新。 選「自訂…」可以自己打，按「試跑」會實際送一個最小
            prompt 過去，成敗就是答案。
          </p>

          {efforts.length > 0 && (
            <>
              <Choice
                label="推理強度"
                value={current.cli.effort}
                options={efforts}
                emptyLabel="（CLI 預設）"
                onChange={(effort) => save({ ...current, cli: { ...current.cli, effort } })}
              />
              <p className="muted hint">
                這個比換模型更有感。出題不太需要深度推理，但 CLI 的預設值常常是高的——
                實測把一題從 98 秒降到 37 秒，主要就是靠這裡跟修掉重試迴圈。
              </p>
            </>
          )}
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
