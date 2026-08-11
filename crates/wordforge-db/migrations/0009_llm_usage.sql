-- 每一次 LLM 呼叫的用量。
--
-- token 數是**選填**，因為拿不拿得到取決於後端：
--
--   HTTP API   回應裡就有 usage，數字是真的
--   claude -p  用 --output-format text，輸出只有內容，沒有用量
--   codex exec 同上
--
-- 所以字元數是必填而 token 是選填。字元數雖然不等於 token，
-- 但它是**每個後端都量得到**的東西，而且要回答「我今天用了多少」
-- 已經夠了。把字元數乘個係數謊報成 token 才是真的沒有用。
CREATE TABLE llm_call (
    id              INTEGER PRIMARY KEY,
    profile_id      INTEGER NOT NULL REFERENCES profile (id) ON DELETE CASCADE,
    called_at       TEXT NOT NULL,
    -- 模型名稱，用來分辨是哪個後端跑的
    model           TEXT NOT NULL,
    -- 這次呼叫在做什麼：generate / grade
    purpose         TEXT NOT NULL,
    prompt_chars    INTEGER NOT NULL,
    response_chars  INTEGER NOT NULL,
    -- 後端有回報才有值
    input_tokens    INTEGER,
    output_tokens   INTEGER,
    -- 失敗的呼叫也要記：重試會燒掉額度，看不到的話用量永遠對不上
    ok              INTEGER NOT NULL DEFAULT 1
);

CREATE INDEX idx_llm_call_time ON llm_call (profile_id, called_at);
