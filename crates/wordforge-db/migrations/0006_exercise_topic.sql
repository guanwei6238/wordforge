-- 出過的情境主題。
--
-- 沒記下來就沒辦法輪換：模型自己挑主題時，挑出來的永遠是那幾個
-- （學校生活、天氣、旅行），十篇文章讀起來像同一篇。
ALTER TABLE exercise ADD COLUMN topic TEXT;
