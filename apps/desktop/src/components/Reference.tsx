/**
 * 批改給的參考答案。
 *
 * 一句話常常有兩種都對的說法，而模型每次只挑一個——同一題
 * 「這個標誌的意思是禁止停車」兩次批改分別給了
 * `This sign means no parking.` 與
 * `This sign means that parking is prohibited.`，兩句都對，
 * 但學習者看到的是「上次那樣寫、這次這樣寫」，分不出哪個才是標準。
 *
 * 所以批改要兩種語體一起給。**兩種一樣時只會有正式那一欄**——
 * 為了湊滿兩欄硬寫一句幾乎一樣的話，會讓人以為它們有語體差別。
 *
 * 這個元件同時要處理三種形狀，因為資料庫裡三種都存在：
 *
 * ```text
 * 正式 + 口語   兩種說法真的不一樣    → 兩行都顯示
 * 只有正式      沒有語體差別          → 顯示成單純的「參考」
 * 只有 reference 舊的批改紀錄，或選擇題 → 同上
 * ```
 */
export default function Reference({
  reference,
  formal,
}: {
  reference: string | null;
  formal: string | null;
}) {
  // 舊紀錄只有 `reference`，那時它就是唯一的參考答案，語氣不明——
  // 這時候它才是「主答案」，不是口語版
  const main = formal ?? reference;
  const casual = formal ? reference : null;
  if (!main) return null;

  return (
    <span className="reference">
      <span className="muted">
        {casual ? "參考（正式）：" : "參考："}
        {main}
      </span>
      {casual && <span className="muted">（口語）：{casual}</span>}
    </span>
  );
}
