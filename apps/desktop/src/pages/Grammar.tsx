import { useCallback, useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  checkDetector,
  currentLanguages,
  deleteGrammar,
  errorMessage,
  explainGrammar,
  type Detector,
  type DetectorCheck,
  type GrammarExample,
  type GrammarView,
  importGrammar,
  isDetectable,
  isGrammarKnown,
  KIND_PATTERN,
  KIND_POINT,
  languageName,
  listGrammar,
  type PatternReport,
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
 * 這張表同時裝兩種東西，而它們要的粒度相反。
 *
 * `point` 是批改時的**錯誤標籤**，要粗——切太細的話每個標籤各錯一次，
 * FSRS 排不動。`pattern` 是教學用的**句型**，要細——「條件句第二類」
 * 跟「第三類」是兩課。所以清單分開看，出題時也分開用。
 */
type Kind = "all" | "point" | "pattern";

const KINDS: { id: Kind; label: string }[] = [
  { id: "all", label: "全部" },
  { id: "point", label: "錯誤標籤" },
  { id: "pattern", label: "句型" },
];

/** 匯入或草擬的結果講成一句話。**被丟掉的一定要說出來。** */
function reportText(report: PatternReport): string {
  const parts = [`寫入 ${report.written} 筆`];
  if (report.detectable > 0) parts.push(`其中 ${report.detectable} 筆偵測得到`);
  if (report.rejected.length > 0) {
    parts.push(`${report.rejected.length} 條偵測規則沒通過自我檢查，已略過`);
  }
  return parts.join("，");
}

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
  const [kind, setKind] = useState<Kind>("all");
  const [rejected, setRejected] = useState<string[]>([]);

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
        if (kind !== "all" && g.kind !== kind) return false;
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
    [items, filter, kind],
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
      const report = await importGrammar(path);
      setNotice(reportText(report));
      setRejected(report.rejected);
      await refresh();
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  const hasPatterns = items.some((g) => g.kind === KIND_PATTERN);

  return (
    <div className="grammar">
      <section className="panel">
        <div className="row title-row">
          <h2>{languageName(langs.target)}文法</h2>
          {items.length === 0 && <span className="muted">還沒有任何文法點</span>}
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

        {hasPatterns && (
          <div className="row">
            {KINDS.map((k) => (
              <button
                key={k.id}
                className={kind === k.id ? "tab active" : "tab"}
                onClick={() => setKind(k.id)}
              >
                {k.label}
              </button>
            ))}
          </div>
        )}

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
        {rejected.length > 0 && (
          <details className="muted hint">
            <summary>{rejected.length} 條偵測規則被略過（點開看原因）</summary>
            <ul>
              {rejected.map((r, i) => (
                <li key={i}>{r}</li>
              ))}
            </ul>
            <p>
              被略過的那幾條<strong>不會有任何作用</strong>：那個句型偵測不到，
              難度上限對它不生效。這裡列出來是因為它壞掉的樣子看起來完全正常。
            </p>
          </details>
        )}
        {error && <p className="error">{error}</p>}
      </section>


      {adding && (
        <GrammarEditor
          initial={null}
          defaultKind={kind === "pattern" ? KIND_PATTERN : KIND_POINT}
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
                  {g.kind === KIND_PATTERN && !isDetectable(g) && (
                    <span className="tag">偵測不到</span>
                  )}
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
                defaultKind={current.kind}
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
        {item.kind === KIND_PATTERN && <span className="tag">句型</span>}
      </header>

      {item.kind === KIND_PATTERN &&
        (isDetectable(item) ? (
          <p className="muted hint">
            這個句型<strong>偵測得到</strong>：出題時系統會實際比對句子有沒有用到它，
            也會從你自己寫的句子判斷你用對了沒有。
          </p>
        ) : (
          <p className="muted hint">
            這個句型<strong>偵測不到</strong>（沒有偵測規則）。它照樣看得到、
            學得到，但難度上限對它不生效，也不會被指派成「這次要練的句型」——
            系統沒辦法確認你真的練到了，猜一個數字只會讓進度失去依據。
          </p>
        ))}

      {item.known_at != null && (
        <p className="muted hint">
          你標記過<strong>我會了</strong>。出題時這個句型會被當成「可以放心用」，
          就算它的級數高過你設定的上限也一樣——你說的話贏過分級的預設值。
        </p>
      )}
      {(item.error_count > 0 || item.correct_count > 0) && (
        <p className="muted hint">
          練習中答對 {item.correct_count} 次、答錯 {item.error_count} 次
          {item.stability != null && `　·　記憶穩定度 ${Math.round(item.stability)} 天`}
        </p>
      )}
      {item.known_at != null && item.error_count > item.correct_count && (
        <p className="muted hint">
          你標記了「我會了」，但練習裡錯得比對得多。系統<strong>不會</strong>
          自動收回你的標記——那是你說的話，只有你能改。覺得不對就按「還要練」。
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
/**
 * 新增或編輯一條定義。
 *
 * ## 為什麼種類要自己選
 *
 * 這張表同時裝兩種東西：批改用的**錯誤標籤**要粗，教學用的**句型**要細。
 * 選錯的後果是安靜的——把「條件句第二類」存成錯誤標籤，它會混進批改
 * prompt 的清單裡稀釋掉排程，而且完全不參與難度上限。
 */
function GrammarEditor({
  initial,
  defaultKind,
  onCancel,
  onSaved,
  onError,
}: {
  initial: GrammarView | null;
  defaultKind: string;
  onCancel: () => void;
  onSaved: () => void;
  onError: (msg: string) => void;
}) {
  const [point, setPoint] = useState(initial?.point ?? "");
  const [name, setName] = useState(initial?.name ?? "");
  const [kind, setKind] = useState(initial?.kind ?? defaultKind);
  const [level, setLevel] = useState(initial?.level ?? "");
  const [ordinal, setOrdinal] = useState<string>(
    initial?.level_ordinal != null ? String(initial.level_ordinal) : "",
  );
  const [explanation, setExplanation] = useState(initial?.explanation ?? "");
  const [examples, setExamples] = useState<GrammarExample[]>(initial?.examples ?? []);
  const [detectors, setDetectors] = useState<Detector[]>(initial?.detectors ?? []);
  const [saving, setSaving] = useState(false);

  const isPattern = kind === KIND_PATTERN;

  async function save() {
    setSaving(true);
    try {
      await saveGrammar({
        point: point.trim(),
        name: name.trim(),
        kind,
        level: level.trim() || null,
        level_ordinal: ordinal.trim() === "" ? null : Number(ordinal),
        explanation: explanation.trim() || null,
        // 空白的例句列不要存進去——使用者按了「加一句」又沒填的殘留
        examples: examples.filter((e) => e.text.trim()),
        // 同理：規則本身是空的就整條丟掉，不然存檔會被控制組擋下來，
        // 而使用者看到的是一則他不知道從哪來的錯誤
        detectors: isPattern ? detectors.filter((d) => d.value.trim()) : [],
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
      <h2>{initial ? `編輯「${initial.name}」` : "新增"}</h2>

      <label>
        這是什麼
        <select value={kind} onChange={(e) => setKind(e.target.value)}>
          <option value={KIND_POINT}>錯誤標籤（批改時用來歸類你犯的錯）</option>
          <option value={KIND_PATTERN}>句型（教學用，會進難度上限）</option>
        </select>
      </label>
      <p className="muted hint">
        {isPattern ? (
          <>
            句型要<strong>細</strong>：「條件句第二類」跟「第三類」是兩課，
            混成一個就練不到重點。句型會照級數決定你現在學不學得到，
            也會被指派成「這次要練的句型」。
          </>
        ) : (
          <>
            錯誤標籤要<strong>粗</strong>：批改時模型從這份清單挑一個來歸類你的錯，
            切太細的話每個標籤各錯一次，排程就推不動了。
            這一類<strong>不參與</strong>句子難度上限。
          </>
        )}
      </p>

      <label>
        識別碼
        <input
          value={point}
          onChange={(e) => setPoint(e.target.value)}
          disabled={initial != null}
          placeholder={isPattern ? "there-be" : "te-form"}
        />
      </label>
      <p className="muted hint">
        英數與連字號，例如 <code>there-be</code>、<code>subject-verb-agreement</code>。
        批改與排程都靠這個識別碼，所以建好之後不能改——
        改了會跟既有的練習紀錄對不上。
      </p>

      <label>
        名稱
        <input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder={isPattern ? "there is / there are" : "て形"}
        />
      </label>

      <div className="row">
        <label>
          等級名稱
          <input
            value={level}
            onChange={(e) => setLevel(e.target.value)}
            placeholder={isPattern ? "國中七年級" : "N5 / A2（可留空）"}
          />
        </label>
        {isPattern && (
          <label>
            排第幾級
            <input
              type="number"
              min={1}
              max={99}
              value={ordinal}
              placeholder="留空＝不分級"
              onChange={(e) => setOrdinal(e.target.value)}
            />
          </label>
        )}
      </div>
      {isPattern && (
        <p className="muted hint">
          名稱只是顯示用的，<strong>程式看的是級數</strong>——字串沒有順序，
          比不出「國中比國小難」。國小填 1、國中填 2、高中填 3，
          設定頁的程度選單就照這個順序排。分幾級由你決定。
          <br />
          留空的話這個句型<strong>不參與難度上限</strong>：我們不知道它難不難，
          猜一個等於憑空造出一條規則。
        </p>
      )}

      <label className="stacked">
        講解
        <textarea
          rows={6}
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

      {isPattern && (
        <>
          <div className="row title-row">
            <strong>偵測規則</strong>
            <button
              onClick={() =>
                setDetectors((x) => [
                  ...x,
                  { type: "regex", value: "", positive: [""], negative: [""] },
                ])
              }
            >
              加一條
            </button>
          </div>
          <p className="muted hint">
            這是整個難度上限成立的地方：沒有規則的句型<strong>偵測不到</strong>，
            系統擋不住超綱的句子，也說不出你有沒有真的練到它。
            <br />
            「一定要命中」的例句是<strong>控制組</strong>，不是裝飾——
            一條打錯字的規則什麼都抓不到，於是檢查永遠通過，而畫面上完全正常。
            所以存檔時會拿這些例句實跑一次，過不了就存不進去。
          </p>
          {detectors.length === 0 && (
            <p className="muted hint">還沒有規則。按「加一條」開始寫。</p>
          )}
          {detectors.map((d, i) => (
            <DetectorEditor
              key={i}
              index={i}
              value={d}
              onChange={(next) => setDetectors((x) => x.map((v, j) => (j === i ? next : v)))}
              onRemove={() => setDetectors((x) => x.filter((_, j) => j !== i))}
            />
          ))}
        </>
      )}

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

/**
 * 一條偵測規則，含當場試打。
 *
 * ## 為什麼試打要繞到後端
 *
 * 瀏覽器的 `RegExp` 跟 Rust 的 `regex` crate **不是同一種方言**：
 * JS 支援 lookahead `(?=)` 與反向參照，Rust 這邊完全不支援。在前端試過
 * 就存進去的話，會拿到一條「編輯器裡看起來好好的、存進去卻編譯失敗」
 * 的規則，而它失敗的樣子是安靜的。
 *
 * 這裡呼叫的是跟存檔、匯入完全同一個函式——這裡說會過，存檔就一定會過。
 */
function DetectorEditor({
  index,
  value,
  onChange,
  onRemove,
}: {
  index: number;
  value: Detector;
  onChange: (next: Detector) => void;
  onRemove: () => void;
}) {
  const [sentence, setSentence] = useState("");
  const [result, setResult] = useState<DetectorCheck | null>(null);
  const [busy, setBusy] = useState(false);

  async function test() {
    setBusy(true);
    try {
      setResult(await checkDetector(value, sentence));
    } catch (e) {
      setResult({ ok: false, problem: errorMessage(e), matched: null, matched_text: null });
    } finally {
      setBusy(false);
    }
  }

  /** 例句的清單編輯：加一句、改一句、移除一句 */
  function editList(field: "positive" | "negative", next: string[]) {
    onChange({ ...value, [field]: next });
  }

  function exampleRows(field: "positive" | "negative", label: string, hint: string) {
    const list = value[field];
    return (
      <>
        <div className="row title-row">
          <span className="muted">
            {label}
            <span className="muted">　{hint}</span>
          </span>
          <button onClick={() => editList(field, [...list, ""])}>加一句</button>
        </div>
        {list.map((ex, i) => (
          <div key={i} className="row">
            <input
              value={ex}
              placeholder={field === "positive" ? "There is a book." : "I put it there."}
              onChange={(e) =>
                editList(
                  field,
                  list.map((v, j) => (j === i ? e.target.value : v)),
                )
              }
            />
            <button onClick={() => editList(field, list.filter((_, j) => j !== i))}>移除</button>
          </div>
        ))}
      </>
    );
  }

  return (
    <div className="panel nested">
      <div className="row title-row">
        <strong>規則 {index + 1}</strong>
        <button onClick={onRemove}>移除這條</button>
      </div>

      <label className="stacked">
        比對規則（Rust regex，不分大小寫）
        <input
          value={value.value}
          placeholder="\bthere\s+(is|are)\b"
          onChange={(e) => onChange({ ...value, value: e.target.value })}
        />
      </label>
      <p className="muted hint">
        <strong>不支援</strong> lookahead <code>(?=)</code>、lookbehind{" "}
        <code>(?&lt;=)</code> 與反向參照 <code>\1</code>——用了會編譯失敗。
        單字用 <code>\b</code> 圈起來（不然 <code>\bif\b</code> 會命中 life），
        跨詞用 <code>[^.?!]*</code> 連接、<strong>不要用</strong> <code>.*</code>
        （它會跨過句號，把兩句話當成一句）。
      </p>

      {exampleRows("positive", "一定要命中", "控制組，至少一句")}
      {exampleRows("negative", "一定不能命中", "挑「像但不是」的句子")}

      <div className="row">
        <input
          value={sentence}
          placeholder="試打一句看看…"
          onChange={(e) => setSentence(e.target.value)}
        />
        <button onClick={test} disabled={busy || !value.value.trim()}>
          {busy ? "試跑中…" : "試試看"}
        </button>
      </div>
      {result &&
        (!result.ok ? (
          <p className="error">{result.problem}</p>
        ) : result.matched == null ? (
          <p className="ok">規則本身沒問題（例句都通過了）。填一句話可以再試打看看。</p>
        ) : result.matched ? (
          <p className="ok">
            命中了：「{result.matched_text}」。
            這句話會被判定成這個句型。
          </p>
        ) : (
          <p className="muted">沒有命中。這句話不會被判定成這個句型。</p>
        ))}
    </div>
  );
}
