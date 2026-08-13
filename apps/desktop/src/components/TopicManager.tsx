import { useCallback, useEffect, useState } from "react";
import {
  deleteTopic,
  errorMessage,
  listTopics,
  saveTopic,
  type Topic,
  TOPIC_KINDS,
} from "../api";

/**
 * 情境主題管理。
 *
 * 出題時會從這裡挑一個題材，並避開最近幾次用過的——不指定的話，
 * 模型永遠寫校園生活與天氣，十篇讀起來像同一篇。
 *
 * 內建那份只是起點。準備多益的人不需要「校園生活：課程、考試、社團」，
 * 醫生要練的科別情境一個都沒有，所以整份都能改。
 */
export default function TopicManager() {
  const [topics, setTopics] = useState<Topic[]>([]);
  const [editing, setEditing] = useState<Topic | null>(null);
  const [adding, setAdding] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setTopics(await listTopics());
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  async function run(action: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await action();
      await load();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  const usable = topics.filter((t) => t.enabled).length;

  return (
    <section className="panel">
      <h2>情境主題</h2>
      <p className="muted hint">
        出題時會從這裡挑一個題材，並避開最近幾次用過的。沒有這份清單的話，
        模型永遠寫校園生活與天氣，讀起來每篇都很像。
        內建的只是起點——改成你真正要練的場合（面試、看診、客服信）會有用得多。
      </p>

      {usable === 0 && topics.length > 0 && (
        <p className="muted hint">
          目前全部都停用了。這樣不會出錯，出題時就是不指定主題，
          題材由模型自己決定。
        </p>
      )}

      <button onClick={() => setAdding(true)} disabled={busy || adding}>
        新增主題
      </button>

      {adding && (
        <TopicForm
          onCancel={() => setAdding(false)}
          onSave={async (draft) => {
            await run(() => saveTopic(draft));
            setAdding(false);
          }}
        />
      )}

      <ul className="topics">
        {topics.map((t) =>
          editing?.id === t.id ? (
            <li key={t.id}>
              <TopicForm
                initial={t}
                onCancel={() => setEditing(null)}
                onSave={async (draft) => {
                  await run(() => saveTopic({ ...draft, id: t.id, origin: t.origin }));
                  setEditing(null);
                }}
              />
            </li>
          ) : (
            <li key={t.id} className={t.enabled ? undefined : "muted"}>
              <div className="topic-head">
                <strong>{t.text}</strong>
                {t.kinds.length === 0 ? (
                  <span className="tag">全部題型</span>
                ) : (
                  t.kinds.map((k) => (
                    <span className="tag" key={k}>
                      {TOPIC_KINDS.find((o) => o.value === k)?.label ?? k}
                    </span>
                  ))
                )}
                {!t.enabled && <span className="tag">已停用</span>}
              </div>
              <div className="topic-actions">
                <button
                  onClick={() => run(() => saveTopic({ ...t, enabled: !t.enabled }))}
                  disabled={busy}
                >
                  {t.enabled ? "停用" : "啟用"}
                </button>
                <button onClick={() => setEditing(t)} disabled={busy}>
                  編輯
                </button>
                <button onClick={() => run(() => deleteTopic(t.id))} disabled={busy}>
                  刪除
                </button>
              </div>
            </li>
          ),
        )}
      </ul>

      {topics.length === 0 && <p className="muted">還沒有主題。</p>}
      {error && <p className="error">{error}</p>}
    </section>
  );
}

/**
 * 新增／編輯一個主題。
 *
 * 題型是複選，全不選代表「全部題型都適用」——那是大多數情況，
 * 所以預設不勾。會需要限定的是體裁類的題材：「報導一則虛構的地方新聞」
 * 對閱讀成立，拿去出翻譯題就是歪的。
 */
function TopicForm({
  initial,
  onSave,
  onCancel,
}: {
  initial?: Topic;
  onSave: (draft: { text: string; kinds: string[]; enabled: boolean }) => Promise<void>;
  onCancel: () => void;
}) {
  const [text, setText] = useState(initial?.text ?? "");
  const [kinds, setKinds] = useState<string[]>(initial?.kinds ?? []);
  const [saving, setSaving] = useState(false);

  function toggle(kind: string) {
    setKinds((now) =>
      now.includes(kind) ? now.filter((k) => k !== kind) : [...now, kind],
    );
  }

  return (
    <div className="topic-form">
      <label>
        主題
        <input
          type="text"
          value={text}
          placeholder="職場：面試、開會、寫信"
          onChange={(e) => setText(e.target.value)}
        />
      </label>
      <p className="muted hint">
        這段文字會直接進 prompt，寫具體一點比較有用。
        「旅行」不如「旅行：訂房、機場、迷路」。
      </p>

      <fieldset className="topic-kinds">
        <legend>用在哪些題型</legend>
        {TOPIC_KINDS.map((k) => (
          <label key={k.value}>
            <input
              type="checkbox"
              checked={kinds.includes(k.value)}
              onChange={() => toggle(k.value)}
            />
            {k.label}
          </label>
        ))}
        <p className="muted hint">全部不勾就是每種題型都可能用到。</p>
      </fieldset>

      <button
        disabled={saving || text.trim().length === 0}
        onClick={async () => {
          setSaving(true);
          try {
            await onSave({ text: text.trim(), kinds, enabled: initial?.enabled ?? true });
          } finally {
            setSaving(false);
          }
        }}
      >
        {saving ? "儲存中…" : "儲存"}
      </button>
      <button onClick={onCancel} disabled={saving}>
        取消
      </button>
    </div>
  );
}
