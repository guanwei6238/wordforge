/** 練習紀錄與教材選單：做過的題目整份叫回來重做，批改照常走一次。 */
import { useState } from "react";
import {
  type ExerciseSummary,
  type Material,
  type ExerciseKind,
} from "../../api";

/** 練習紀錄。做過的題目整份叫回來重做，批改照常走一次。 */
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
}) {
  // 刪除要二次確認，但不用彈窗——按一下變成「確定刪除」，
  // 按別的地方或再點一次別份就恢復
  const [confirming, setConfirming] = useState<number | null>(null);
  const pages = Math.max(1, Math.ceil(total / HISTORY_PAGE));

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
              {it.coverage != null && `　·　覆蓋率 ${Math.round(it.coverage * 100)}%`}
            </div>
            <div className="history-actions">
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
