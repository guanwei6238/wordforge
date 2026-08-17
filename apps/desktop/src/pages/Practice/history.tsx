/** 練習紀錄與教材選單：做過的題目整份叫回來重做，批改照常走一次。 */
import { useCallback, useEffect, useState } from "react";
import ExerciseBoard from "./board";
import {
  type AttemptView,
  deleteAttempt,
  errorMessage,
  type ExerciseView,
  type ExerciseKind,
  type ExerciseSummary,
  getStudySettings,
  listAttempts,
  loadExercise,
  type Material,
  type ProfileLanguages,
  currentLanguages,
  deleteSentenceAttempts,
  listSentenceAttempts,
  REVIEW_LOG_PAGE,
  type SentenceAttemptPage,
} from "../../api";
import Corrections from "../../components/Corrections";
import Reference from "../../components/Reference";

/** 練習紀錄一頁幾筆。一頁塞太多就等於沒有分頁。 */
export const HISTORY_PAGE = 10;

export function History({
  items,
  total,
  page,
  labels,
  disabled,
  currentId,
  onPage,
  onRedo,
  onDelete,
  onChanged,
  onOpenChange,
}: {
  items: ExerciseSummary[];
  total: number;
  page: number;
  labels: Record<ExerciseKind, string>;
  disabled: boolean;
  currentId: number | null;
  onPage: (page: number) => void;
  onRedo: (id: number) => void;
  onDelete: (id: number) => void;
  /** 重寫送出後要讓外面的清單也跟著更新——分數變了 */
  onChanged: () => void;
  /** 攤開某一份時通知外面：詳情裡是完整的練習畫面，要用寬版 */
  onOpenChange: (open: boolean) => void;
}) {
  // 刪除要二次確認，但不用彈窗——按一下變成「確定刪除」，
  // 按別的地方或再點一次別份就恢復
  const [confirming, setConfirming] = useState<number | null>(null);
  // 點進去看，不是就地展開。做過十次的練習攤在清單裡會長到捲不完，
  // 而且旁邊那幾份的資訊全被推走了
  const [opened, setOpened] = useState<number | null>(null);
  // 練習與複習分開看。兩件事的單位不一樣——一邊是「一份題目做過幾次」，
  // 一邊是「今天複習了哪幾句」，混在同一張清單裡兩邊都會被對方稀釋。
  const [tab, setTab] = useState<"exercises" | "reviews">("exercises");

  function open(exerciseId: number | null) {
    setOpened(exerciseId);
    onOpenChange(exerciseId != null);
  }
  const pages = Math.max(1, Math.ceil(total / HISTORY_PAGE));

  if (opened != null) {
    return (
      <AttemptDetail
        exerciseId={opened}
        title={items.find((i) => i.exercise_id === opened)?.title ?? "這份練習"}
        onBack={() => open(null)}
        onChanged={onChanged}
      />
    );
  }

  const tabs = (
    <div className="row review-modes">
      <button
        className={tab === "exercises" ? "tab active" : "tab"}
        onClick={() => setTab("exercises")}
      >
        練習{total > 0 && `（${total}）`}
      </button>
      <button
        className={tab === "reviews" ? "tab active" : "tab"}
        onClick={() => setTab("reviews")}
      >
        複習句子
      </button>
    </div>
  );

  if (tab === "reviews") {
    return (
      <section className="panel">
        <h2>紀錄</h2>
        {tabs}
        <ReviewLog labels={labels} />
      </section>
    );
  }

  if (total === 0) {
    return (
      <section className="panel">
        <h2>紀錄</h2>
        {tabs}
        <p className="muted">還沒有做過練習。做完的題目會留在這裡，可以整份再做一次。</p>
      </section>
    );
  }

  return (
    <section className="panel">
      <h2>紀錄</h2>
      {tabs}
      <ul className="history">
        {items.map((it) => (
          <li key={it.exercise_id} className={it.exercise_id === currentId ? "current" : ""}>
            <div>
              <span className="tag">{labels[it.kind] ?? it.kind}</span>
              <span className="history-title">{it.title}</span>
            </div>
            <div className="muted history-meta">
              {formatWhen(it.created_at)}
              {it.score != null ? `　·　${Math.round(it.score)} 分` : "　·　沒作答"}
              {it.attempts > 1 && `　·　做過 ${it.attempts} 次`}
              {/* 「還有幾題沒全對」是重寫的入口，也是「做到 100 分」的進度。
                  pending 是 null 時說不出來，就不要猜成全對 */}
              {it.pending != null &&
                (it.pending > 0 ? (
                  `　·　還有 ${it.pending} 題沒全對`
                ) : (
                  <span className="ok">　·　全對</span>
                ))}
              {it.coverage != null && `　·　覆蓋率 ${Math.round(it.coverage * 100)}%`}
            </div>
            <div className="history-actions">
              <button onClick={() => open(it.exercise_id)}>看作答</button>
              <button onClick={() => onRedo(it.exercise_id)} disabled={disabled}>
                再做一次
              </button>
              {confirming === it.exercise_id ? (
                <>
                  <button
                    className="destructive"
                    onClick={() => {
                      setConfirming(null);
                      onDelete(it.exercise_id);
                    }}
                    disabled={disabled}
                  >
                    確定刪除
                  </button>
                  <button onClick={() => setConfirming(null)}>取消</button>
                </>
              ) : (
                <button onClick={() => setConfirming(it.exercise_id)} disabled={disabled}>
                  刪除
                </button>
              )}
            </div>
          </li>
        ))}
      </ul>

      {pages > 1 && (
        <div className="row pager">
          <button onClick={() => onPage(page - 1)} disabled={page === 0 || disabled}>
            上一頁
          </button>
          <span className="muted">
            第 {page + 1} / {pages} 頁　·　共 {total} 份
          </span>
          <button onClick={() => onPage(page + 1)} disabled={page + 1 >= pages || disabled}>
            下一頁
          </button>
        </div>
      )}

      <p className="muted hint">
        重做的是同一份題目，不會再花一次出題的額度；送出後一樣由模型批改，
        舊的那次批改也留著。刪掉的話那份題目與所有作答一起消失，沒有復原。
      </p>
    </section>
  );
}

/**
 * 複習句子的紀錄，新的在前。
 *
 * 跟練習紀錄分開的兩張表、兩個查詢，理由是**單位不一樣**：練習紀錄的
 * 一列是「一份題目」，複習紀錄的一列是「一句」。複習曾經借用練習的
 * `attempt` 表，於是複習三句就在那份練習底下長出三筆「第 N 次」，
 * 清單上的「做過 12 次」其實是複習了 12 句。
 *
 * 這裡不提供「再做一次」：一句每天只練一次，那正是這條排程的規則。
 */
function ReviewLog({ labels }: { labels: Record<ExerciseKind, string> }) {
  const [page, setPage] = useState(0);
  const [data, setData] = useState<SentenceAttemptPage | null>(null);
  const [error, setError] = useState<string | null>(null);
  // 刪除要二次確認，但不用彈窗——按一下變成「確定刪除」，跟練習紀錄一致。
  // 認的是那一組的時間戳，因為一列就是一次送出。
  const [confirming, setConfirming] = useState<string | null>(null);
  // 刪完要重讀這一頁。用計數器而不是直接改 data：總數與分頁都會變
  const [reload, setReload] = useState(0);

  useEffect(() => {
    let cancelled = false;
    void listSentenceAttempts(REVIEW_LOG_PAGE, page * REVIEW_LOG_PAGE)
      .then((got) => {
        if (!cancelled) setData(got);
      })
      .catch((e) => {
        if (!cancelled) setError(errorMessage(e));
      });
    return () => {
      cancelled = true;
    };
  }, [page, reload]);

  if (error) return <p className="error">{error}</p>;
  if (!data) return <p className="muted">載入中…</p>;
  if (data.total === 0) {
    return (
      <p className="muted">
        還沒有複習過句子。翻譯題寫錯的句子隔天會回到複習頁，寫過就會留在這裡。
      </p>
    );
  }

  const pages = Math.max(1, Math.ceil(data.total / REVIEW_LOG_PAGE));

  async function remove(ids: number[]) {
    setConfirming(null);
    try {
      await deleteSentenceAttempts(ids);
      // 刪到某一頁只剩空的時候要退回上一頁，不然畫面是一片空白
      if (data && data.items.length === 1 && page > 0) setPage(page - 1);
      else setReload((n) => n + 1);
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  return (
    <>
      <ul className="review-log">
        {data.items.map((batch) => {
          const passed = batch.items.filter((a) => a.correct).length;
          const kind = batch.items[0]?.kind;
          return (
            <li key={batch.created_at}>
              {/* 一列是「一次送出」：那一輪練了幾句、對了幾句。
                  攤成一句一列的話，使用者記得的「剛剛那一次」就被打散了 */}
              <div className="row review-batch-head">
                <span className="muted history-meta">
                  {formatWhen(batch.created_at)}
                  {kind && `　·　${labels[kind] ?? kind}`}
                  　·　{batch.items.length} 句
                  {passed > 0 && <span className="ok">　·　{passed} 句寫對</span>}
                </span>
                {confirming === batch.created_at ? (
                  <span className="row">
                    <button
                      className="destructive"
                      onClick={() => void remove(batch.items.map((a) => a.id))}
                    >
                      確定刪除
                    </button>
                    <button onClick={() => setConfirming(null)}>取消</button>
                  </span>
                ) : (
                  <button onClick={() => setConfirming(batch.created_at)}>刪除</button>
                )}
              </div>

              <ol className="review-batch">
                {batch.items.map((a) => (
                  <li key={a.id}>
                    {/* 題目那份練習被刪掉時 source 是空的。那時候仍然要列出
                        這一筆——他寫過的東西不該因為題目沒了就整列消失 */}
                    <p className="prompt">
                      {a.correct ? <span className="ok">✓ </span> : <span className="error">✗ </span>}
                      {a.source || <span className="muted">（題目已刪除）</span>}
                    </p>
                    <p className="attempt-mine">{a.answer}</p>
                    <p>
                      <Reference reference={a.reference} formal={a.reference_formal} />
                    </p>
                    {a.comment && <p className="muted">{a.comment}</p>}
                    <Corrections items={a.corrections} />
                  </li>
                ))}
              </ol>
            </li>
          );
        })}
      </ul>

      {pages > 1 && (
        <div className="row pager">
          <button onClick={() => setPage(page - 1)} disabled={page === 0}>
            上一頁
          </button>
          <span className="muted">
            第 {page + 1} / {pages} 頁　·　複習過 {data.total} 次、共 {data.sentences} 句
          </span>
          <button onClick={() => setPage(page + 1)} disabled={page + 1 >= pages}>
            下一頁
          </button>
        </div>
      )}

      <p className="muted hint">
        刪除只會拿掉這筆紀錄，不影響排程——那一句還沒寫對的話，明天照樣會回到複習頁。
      </p>
    </>
  );
}

/**
 * 一份練習做過的每一次：你當時寫了什麼、模型當時怎麼講。
 *
 * 這些資料一直都存著（`attempt` 表），只是從來沒有讀出來過——
 * 做完一份練習關掉畫面，寫過什麼就再也叫不回來，而重做同一份時
 * 最想看的正是「上次我是怎麼寫的」。
 *
 * 一次只看一次的作答，右邊那欄切換。全部攤開的話，做過十次的練習
 * 會是一大片重複的題目，而使用者要比的是「這次跟上次差在哪」。
 *
 * 題目本身要另外撈（`attempt` 只存作答，不存題目），所以這裡同時
 * 要兩份資料才拼得出「第 1 題問什麼、你答什麼」。
 */
function AttemptDetail({
  exerciseId,
  title,
  onBack,
  onChanged,
}: {
  exerciseId: number;
  title: string;
  onBack: () => void;
  onChanged: () => void;
}) {
  const [exercise, setExercise] = useState<ExerciseView | null>(null);
  // 解析階段點文章裡的字會查釋義，那個功能在紀錄裡一樣有用
  const [lookup, setLookup] = useState<string | null>(null);
  // 題型名稱與文章字級跟練習頁走同一份設定
  const [langs, setLangs] = useState<ProfileLanguages>({ native: "zh-TW", target: "en" });
  const [fontSize, setFontSize] = useState(16);
  const [attempts, setAttempts] = useState<AttemptView[] | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<number | null>(null);

  const load = useCallback(async () => {
    try {
      const [loaded, past] = await Promise.all([
        loadExercise(exerciseId),
        listAttempts(exerciseId),
      ]);
      setExercise(loaded);
      setAttempts(past);
      // 預設看最新那次：那是「我現在做到哪」，也是唯一能重寫的一次
      setSelected((now) =>
        now != null && past.some((a) => a.attempt_id === now)
          ? now
          : (past[past.length - 1]?.attempt_id ?? null),
      );
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, [exerciseId]);

  useEffect(() => {
    void load();
    void currentLanguages()
      .then(setLangs)
      .catch(() => {});
    void getStudySettings()
      .then((s) => setFontSize(s.reading_font_size))
      .catch(() => {});
  }, [load]);

  async function remove(attemptId: number) {
    setConfirming(null);
    try {
      await deleteAttempt(attemptId);
      await load();
      onChanged();
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  const current = attempts?.find((a) => a.attempt_id === selected) ?? null;

  return (
    <section className="panel">
      <div className="row attempt-head">
        <button onClick={onBack}>← 回到紀錄</button>
        <h2>{title}</h2>
      </div>

      {error && <p className="error">{error}</p>}
      {!attempts || !exercise ? (
        <p className="muted">載入中…</p>
      ) : attempts.length === 0 ? (
        <p className="muted">這份出過題但沒有作答紀錄。</p>
      ) : (
        <div className="attempt-detail">
          <div className="attempt-main">
            {current && (
              // 還原成「當時做完、批改完」的樣子，用的是練習頁那一份畫面。
              // 自己再寫一套逐題摘要只會有兩份互相漂移的版面，而且那一套
              // 永遠比較簡陋——沒有文章、沒有選項、沒有生字表。
              <ExerciseBoard
                exercise={exercise}
                langs={langs}
                fontSize={fontSize}
                onFontSize={() => {}}
                answers={current.answer?.answers ?? []}
                choices={current.answer?.choices ?? []}
                marked={current.answer?.marked_unknown ?? []}
                lookup={lookup}
                feedback={current.feedback}
                setLookup={setLookup}
                addedLater={[]}
                onAddLater={() => {}}
                setAnswers={() => {}}
                setChoices={() => {}}
                onToggleMark={() => {}}
                onSubmit={() => {}}
                busy={null}
                unanswered={0}
                materials={[]}
                materialId={null}
                setMaterialId={() => {}}
              />
            )}
          </div>

          <aside className="attempt-picker">
            {attempts.map((a, n) => (
              <div
                key={a.attempt_id}
                className={a.attempt_id === selected ? "current" : ""}
              >
                <button className="pick" onClick={() => setSelected(a.attempt_id)}>
                  <span>第 {n + 1} 次</span>
                  <span className="muted">
                    {formatWhen(a.created_at)}
                    {a.score != null && `　${Math.round(a.score)} 分`}
                  </span>
                </button>
                {confirming === a.attempt_id ? (
                  <span className="row">
                    <button
                      className="destructive"
                      onClick={() => void remove(a.attempt_id)}
                    >
                      確定
                    </button>
                    <button onClick={() => setConfirming(null)}>取消</button>
                  </span>
                ) : (
                  <button onClick={() => setConfirming(a.attempt_id)}>刪除</button>
                )}
              </div>
            ))}
          </aside>
        </div>
      )}
    </section>
  );
}

/** 資料庫存的是 RFC 3339 的 UTC，顯示成本地時間。 */
export function formatWhen(iso: string): string {
  const d = new Date(iso.endsWith("Z") ? iso : `${iso}Z`);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString(undefined, {
    month: "numeric",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function MaterialPicker({
  materials,
  value,
  onChange,
  disabled,
}: {
  materials: Material[];
  value: number | null;
  onChange: (id: number | null) => void;
  disabled: boolean;
}) {
  return (
    <label>
      取材範圍
      <select
        value={value ?? ""}
        onChange={(e) => onChange(e.target.value ? Number(e.target.value) : null)}
        disabled={disabled}
      >
        <option value="">自由出題</option>
        {materials.map((m) => (
          <option key={m.id} value={m.id}>
            只從《{m.title}》出題
          </option>
        ))}
      </select>
    </label>
  );
}
