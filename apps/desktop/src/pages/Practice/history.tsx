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
} from "../../api";

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

  if (total === 0) {
    return (
      <section className="panel">
        <h2>練習紀錄</h2>
        <p className="muted">還沒有做過練習。做完的題目會留在這裡，可以整份再做一次。</p>
      </section>
    );
  }

  return (
    <section className="panel">
      <h2>練習紀錄</h2>
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
