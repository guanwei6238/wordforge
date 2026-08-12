import { useCallback, useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  currentLanguages,
  deleteGrammar,
  errorMessage,
  explainGrammar,
  type GrammarExample,
  type GrammarView,
  importGrammar,
  isGrammarKnown,
  languageName,
  listGrammar,
  type ProfileLanguages,
  saveGrammar,
  setGrammarKnown,
} from "../api";
import SpeakButton from "../components/SpeakButton";

type Filter = "all" | "learning" | "known" | "untouched";

const FILTERS: { id: Filter; label: string }[] = [
  { id: "all", label: "全部" },
  { id: "untouched", label: "還沒開始" },
  { id: "learning", label: "在學" },
  { id: "known", label: "已學會" },
];

/**
 * 文法頁：跟單字一樣，自己決定學會了沒有。
 *
 * ## 清單從哪來
 *
 * 存在 `grammar_def` 資料表，不是寫死的常數——「匯入什麼就能學什麼」
 * 對文法跟對字典是同一個承諾。英文有一份內建種子，其他語言開箱是空的，
 * 由使用者匯入或自己加。
 *
 * ## 講解從哪來
 *
 * 沒有可以直接匯入的開源文法書（查過的來源不是授權不明，就是標註規範
 * 而不是教材），所以講解由模型當場生成、存進資料庫，之後可以自己編輯。
 * 生成一次就存起來，開頁不會重打。
 */
export default function Grammar() {
  const [items, setItems] = useState<GrammarView[]>([]);
  const [langs, setLangs] = useState<ProfileLanguages>({ native: "zh-TW", target: "en" });
  const [filter, setFilter] = useState<Filter>("all");
  const [selected, setSelected] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [adding, setAdding] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setItems(await listGrammar());
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
    void currentLanguages().then(setLangs).catch(() => {});
  }, [refresh]);

  const shown = useMemo(
    () =>
      items.filter((g) => {
        switch (filter) {
          case "known":
            return isGrammarKnown(g);
          case "learning":
            return g.state != null && !isGrammarKnown(g);
          case "untouched":
            return g.state == null;
          default:
            return true;
        }
      }),
    [items, filter],
  );

  const current = items.find((g) => g.point === selected) ?? null;

  async function explain(point: string) {
    setBusy(point);
    setError(null);
    try {
      await explainGrammar(point);
      await refresh();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  }

  async function mark(point: string, known: boolean) {
    setError(null);
    try {
      await setGrammarKnown(point, known);
      await refresh();
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  async function remove(point: string) {
    setError(null);
    try {
      await deleteGrammar(point);
      if (selected === point) setSelected(null);
      await refresh();
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  async function pickAndImport() {
    setError(null);
    setNotice(null);
    try {
      const path = await open({
        multiple: false,
        filters: [{ name: "文法清單", extensions: ["json"] }],
      });
      if (typeof path !== "string") return;
      const n = await importGrammar(path);
      setNotice(`匯入了 ${n} 個文法點`);
      await refresh();
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  const knownCount = items.filter(isGrammarKnown).length;

  return (
    <div className="grammar">
      <section className="panel">
        <div className="row title-row">
          <h2>{languageName(langs.target)}文法</h2>
          <span className="muted">
            {items.length > 0
              ? `${knownCount} / ${items.length} 已學會`
              : "還沒有任何文法點"}
          </span>
        </div>

        <div className="row">
          {FILTERS.map((f) => (
            <button
              key={f.id}
              className={filter === f.id ? "tab active" : "tab"}
              onClick={() => setFilter(f.id)}
            >
              {f.label}
            </button>
          ))}
          <span style={{ marginLeft: "auto" }} />
          <button onClick={() => setAdding(true)}>自己加一個</button>
          <button onClick={pickAndImport}>匯入清單…</button>
        </div>

        {items.length === 0 && (
          <p className="muted hint">
            這個語言還沒有文法點。內建的種子只有英文——
            日文的助詞、法文的性數一致這些需要各自的清單，硬套英文的分類
            只會產生垃圾資料，所以寧可留空。
            <br />
            按「匯入清單…」帶一份 JSON 進來，或「自己加一個」慢慢累積。
            格式是 <code>{`[{"point": "te-form", "name": "て形"}]`}</code>，
            只有這兩個欄位是必要的。
          </p>
        )}

        {notice && <p className="ok">{notice}</p>}
        {error && <p className="error">{error}</p>}
      </section>

      {adding && (
        <GrammarEditor
          initial={null}
          onCancel={() => setAdding(false)}
          onSaved={async () => {
            setAdding(false);
            await refresh();
          }}
          onError={setError}
        />
      )}

      {items.length > 0 && (
        <div className="grammar-body">
          <ul className="grammar-list">
            {shown.map((g) => (
              <li key={g.point}>
                <button
                  className={selected === g.point ? "hit selected" : "hit"}
                  onClick={() => {
                    setSelected(g.point === selected ? null : g.point);
                    setEditing(false);
                  }}
                >
                  <span className="hit-word">{g.name}</span>
                  {g.level && <span className="tag">{g.level}</span>}
                  {isGrammarKnown(g) ? (
                    <span className="tag in-deck">已學會</span>
                  ) : g.state != null ? (
                    <span className="tag">在學</span>
                  ) : null}
                  {!g.explanation && <span className="tag">尚未講解</span>}
                </button>
              </li>
            ))}
            {shown.length === 0 && <li className="empty muted">這個篩選沒有東西</li>}
          </ul>

          {current ? (
            editing ? (
              <GrammarEditor
                initial={current}
                onCancel={() => setEditing(false)}
                onSaved={async () => {
                  setEditing(false);
                  await refresh();
                }}
                onError={setError}
              />
            ) : (
              <GrammarDetail
                item={current}
                targetLang={langs.target}
                busy={busy === current.point}
                onExplain={() => explain(current.point)}
                onEdit={() => setEditing(true)}
                onMark={(known) => mark(current.point, known)}
                onDelete={() => remove(current.point)}
              />
            )
          ) : (
            <p className="empty muted">左邊挑一個文法點。</p>
          )}
        </div>
      )}
    </div>
  );
}

function GrammarDetail({
  item,
  targetLang,
  busy,
  onExplain,
  onEdit,
  onMark,
  onDelete,
}: {
  item: GrammarView;
  targetLang: string;
  busy: boolean;
  onExplain: () => void;
  onEdit: () => void;
  onMark: (known: boolean) => void;
  onDelete: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const known = isGrammarKnown(item);

  return (
    <div className="detail">
      <header>
        <h2>{item.name}</h2>
        <span className="tag">{item.point}</span>
        {item.level && <span className="tag">{item.level}</span>}
      </header>

      {(item.error_count > 0 || item.correct_count > 0) && (
        <p className="muted hint">
          練習中答對 {item.correct_count} 次、答錯 {item.error_count} 次
          {item.stability != null && `　·　記憶穩定度 ${Math.round(item.stability)} 天`}
        </p>
      )}

      {item.explanation ? (
        <p className="grammar-explanation">{item.explanation}</p>
      ) : (
        <p className="muted hint">
          還沒有講解。沒有可以直接匯入的開源文法書，所以這一格要嘛請 AI 寫，
          要嘛你自己寫——寫完都存得下來，之後可以再改。
        </p>
      )}

      {item.examples.length > 0 && (
        <ul className="grammar-examples">
          {item.examples.map((ex, i) => (
            <li key={i}>
              <span className="example-text">{ex.text}</span>
              <SpeakButton text={ex.text} lang={targetLang} />
              {ex.translation && <span className="muted"> {ex.translation}</span>}
            </li>
          ))}
        </ul>
      )}

      <div className="row">
        <button onClick={onExplain} disabled={busy}>
          {busy ? (
            <>
              <span className="spinner" aria-hidden="true" /> 講解中…
            </>
          ) : item.explanation ? (
            "請 AI 重寫講解"
          ) : (
            "請 AI 講解"
          )}
        </button>
        <button onClick={onEdit} disabled={busy}>
          自己寫 / 編輯
        </button>
      </div>

      <div className="row">
        <button className="primary" onClick={() => onMark(true)}>
          {known ? "再確認一次會了" : "我會了"}
        </button>
        <button onClick={() => onMark(false)}>還要多練</button>
        <span style={{ marginLeft: "auto" }} />
        {confirming ? (
          <>
            <button className="destructive" onClick={onDelete}>
              確定刪除
            </button>
            <button onClick={() => setConfirming(false)}>取消</button>
          </>
        ) : (
          <button onClick={() => setConfirming(true)}>刪除</button>
        )}
      </div>

      <p className="muted hint">
        「我會了」走的是跟答題一樣的排程——自評與實際作答會匯流到同一個進度，
        不會變成兩套互相打架的狀態。刪除只拿掉這份講解，練習紀錄留著。
      </p>
    </div>
  );
}

/** 新增或編輯一個文法點。 */
function GrammarEditor({
  initial,
  onCancel,
  onSaved,
  onError,
}: {
  initial: GrammarView | null;
  onCancel: () => void;
  onSaved: () => void;
  onError: (msg: string) => void;
}) {
  const [point, setPoint] = useState(initial?.point ?? "");
  const [name, setName] = useState(initial?.name ?? "");
  const [level, setLevel] = useState(initial?.level ?? "");
  const [explanation, setExplanation] = useState(initial?.explanation ?? "");
  const [examples, setExamples] = useState<GrammarExample[]>(initial?.examples ?? []);
  const [saving, setSaving] = useState(false);

  async function save() {
    setSaving(true);
    try {
      await saveGrammar({
        point: point.trim(),
        name: name.trim(),
        level: level.trim() || null,
        explanation: explanation.trim() || null,
        // 空白的例句列不要存進去——使用者按了「加一句」又沒填的殘留
        examples: examples.filter((e) => e.text.trim()),
        origin: initial?.origin || "manual",
      });
      onSaved();
    } catch (e) {
      onError(errorMessage(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <section className="panel">
      <h2>{initial ? `編輯「${initial.name}」` : "新增文法點"}</h2>

      <label>
        識別碼
        <input
          value={point}
          onChange={(e) => setPoint(e.target.value)}
          disabled={initial != null}
          placeholder="te-form"
        />
      </label>
      <p className="muted hint">
        英數與連字號，例如 <code>te-form</code>、<code>subject-verb-agreement</code>。
        批改時模型回報的標籤會收斂到這個識別碼，所以建好之後不能改——
        改了會跟既有的練習紀錄對不上。
      </p>

      <label>
        名稱
        <input value={name} onChange={(e) => setName(e.target.value)} placeholder="て形" />
      </label>
      <label>
        難度標示
        <input
          value={level}
          onChange={(e) => setLevel(e.target.value)}
          placeholder="N5 / A2（可留空）"
        />
      </label>

      <label className="stacked">
        講解
        <textarea
          rows={8}
          value={explanation}
          onChange={(e) => setExplanation(e.target.value)}
          placeholder="什麼時候用、怎麼構成、最容易犯的錯…"
        />
      </label>

      <div className="row title-row">
        <strong>例句</strong>
        <button onClick={() => setExamples((x) => [...x, { text: "", translation: "" }])}>
          加一句
        </button>
      </div>
      {examples.map((ex, i) => (
        <div key={i} className="row">
          <input
            value={ex.text}
            placeholder="目標語例句"
            onChange={(e) =>
              setExamples((x) => x.map((v, j) => (j === i ? { ...v, text: e.target.value } : v)))
            }
          />
          <input
            value={ex.translation ?? ""}
            placeholder="母語翻譯"
            onChange={(e) =>
              setExamples((x) =>
                x.map((v, j) => (j === i ? { ...v, translation: e.target.value } : v)),
              )
            }
          />
          <button onClick={() => setExamples((x) => x.filter((_, j) => j !== i))}>移除</button>
        </div>
      ))}

      <div className="row">
        <button
          className="primary"
          onClick={save}
          disabled={saving || !point.trim() || !name.trim()}
        >
          {saving ? "儲存中…" : "儲存"}
        </button>
        <button onClick={onCancel} disabled={saving}>
          取消
        </button>
        {(!point.trim() || !name.trim()) && (
          <span className="muted">識別碼與名稱是必要的</span>
        )}
      </div>
    </section>
  );
}
