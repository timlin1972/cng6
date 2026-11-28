//! AI 可以呼叫的 cng6 功能(Gemini function calling)。
//!
//! 只在放 API key 的那台執行, 用的是那台的郵件、待辦(整個 cluster 共用)、發票、天氣與 cluster 狀態。
//! 讀信用 BODY.PEEK, 不會把信標成已讀。

use chrono::TimeZone;
use serde_json::{Value, json};

use crate::plugins::mail::imap::{Job, Out};
use crate::plugins::{cluster, invoice, mail, todo, weather};

/// 工具需要的其他 plugin
#[derive(Clone)]
pub struct Deps {
    pub mail: mail::Shared,
    pub todo: todo::Shared,
    pub invoice: invoice::Shared,
    pub weather: weather::Shared,
    pub cluster: cluster::Shared,
}

/// 給 Gemini 的工具說明
pub fn declarations() -> Value {
    json!([{ "functionDeclarations": [
        {
            "name": "search_mail",
            "description": "搜尋使用者的 Gmail(Gmail 搜尋語法, 例如 'from:amazon newer_than:7d'、'is:unread in:inbox'、'發票')。回傳信件清單(新的在前)。",
            "parameters": { "type": "object", "properties": {
                "query": { "type": "string", "description": "Gmail 搜尋條件; 空字串表示所有郵件" },
                "account": { "type": "string", "description": "帳號名稱(選填, 不填就搜尋全部帳號)" },
                "max": { "type": "integer", "description": "最多幾封, 預設 10, 最多 30" }
            }, "required": ["query"] }
        },
        {
            "name": "read_mail",
            "description": "讀一封信的內容(主旨、寄件者、日期、內文、附件名稱)。account 與 id 從 search_mail 的結果取得。",
            "parameters": { "type": "object", "properties": {
                "account": { "type": "string" },
                "id": { "type": "string" }
            }, "required": ["account", "id"] }
        },
        {
            "name": "list_todos",
            "description": "列出待辦清單(整個 cluster 共用)。",
            "parameters": { "type": "object", "properties": {
                "include_done": { "type": "boolean", "description": "是否包含已完成的, 預設 false" }
            } }
        },
        {
            "name": "add_todo",
            "description": "新增一項待辦。",
            "parameters": { "type": "object", "properties": {
                "title": { "type": "string" },
                "priority": { "type": "integer", "description": "1 高, 2 中(預設), 3 低" },
                "due": { "type": "string", "description": "到期日 YYYY-MM-DD(選填)" },
                "notes": { "type": "string", "description": "備註(選填)" }
            }, "required": ["title"] }
        },
        {
            "name": "update_todo",
            "description": "修改待辦: 標成完成/未完成、改內容、優先順序、到期日(空字串清掉)、備註。id 從 list_todos 取得。",
            "parameters": { "type": "object", "properties": {
                "id": { "type": "string" },
                "done": { "type": "boolean" },
                "title": { "type": "string" },
                "priority": { "type": "integer" },
                "due": { "type": "string" },
                "notes": { "type": "string" }
            }, "required": ["id"] }
        },
        {
            "name": "invoice_status",
            "description": "電子發票的對獎狀況: 中獎的發票(獎項、金額、領獎期限、是否已領)、未開獎的數量、找不到號碼的信。"
        },
        {
            "name": "cluster_status",
            "description": "cng6 cluster 裡每台機器的狀態: 是否在線、CPU、記憶體、磁碟、溫度、開機時間、版本。"
        },
        {
            "name": "weather",
            "description": "使用者設定的城市的天氣: 目前天氣與未來幾天的預報、警特報。"
        }
    ]}])
}

/// 工具名稱的中文說明(網頁顯示「正在…」)
pub fn label(name: &str) -> &'static str {
    match name {
        "search_mail" => "搜尋郵件",
        "read_mail" => "讀信",
        "list_todos" => "查看待辦",
        "add_todo" => "新增待辦",
        "update_todo" => "修改待辦",
        "invoice_status" => "查看發票",
        "cluster_status" => "查看機器狀態",
        "weather" => "查看天氣",
        _ => "使用工具",
    }
}

fn local_time(ts: i64) -> String {
    chrono::Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

fn s<'a>(args: &'a Value, k: &str) -> &'a str {
    args[k].as_str().unwrap_or("").trim()
}

/// 執行一個工具, 回傳結果(錯誤也包成 {"error": …} 讓模型知道)
pub async fn run(deps: &Deps, name: &str, args: &Value) -> Value {
    let r = match name {
        "search_mail" => search_mail(deps, args).await,
        "read_mail" => read_mail(deps, args).await,
        "list_todos" => Ok(list_todos(deps, args)),
        "add_todo" => add_todo(deps, args),
        "update_todo" => update_todo(deps, args),
        "invoice_status" => Ok(invoice_status(deps)),
        "cluster_status" => Ok(cluster_status(deps)),
        "weather" => Ok(weather(deps)),
        _ => Err(format!("沒有這個工具: {name}")),
    };
    r.unwrap_or_else(|e| json!({ "error": e }))
}

async fn search_mail(deps: &Deps, args: &Value) -> Result<Value, String> {
    let q = s(args, "query").to_string();
    let max = args["max"].as_u64().unwrap_or(10).clamp(1, 30) as usize;
    let want = s(args, "account");
    let accounts: Vec<String> = deps
        .mail
        .accounts()
        .into_iter()
        .map(|a| a.name)
        .filter(|n| want.is_empty() || n == want)
        .collect();
    if accounts.is_empty() {
        return Err(if want.is_empty() { "還沒有設定 Gmail 帳號".into() } else { format!("沒有帳號 {want}") });
    }
    let mut out = Vec::new();
    for a in accounts {
        match deps.mail.request(&a, Job::List { mailbox: None, q: q.clone(), offset: 0 }).await {
            Ok(Out::List(page)) => {
                let items: Vec<Value> = page
                    .items
                    .iter()
                    .take(max)
                    .map(|m| {
                        json!({
                            "id": m.id, "from": m.from, "from_addr": m.from_addr, "subject": m.subject,
                            "date": local_time(m.ts), "unread": m.unread, "starred": m.starred,
                            "in_inbox": m.inbox, "attachment": m.attach, "labels": m.labels,
                        })
                    })
                    .collect();
                out.push(json!({ "account": a, "total": page.total, "messages": items }));
            }
            Ok(_) => out.push(json!({ "account": a, "error": "回應格式不對" })),
            Err(e) => out.push(json!({ "account": a, "error": e })),
        }
    }
    Ok(json!({ "results": out }))
}

/// 內文最多給模型多少字
const MAX_BODY: usize = 20_000;

async fn read_mail(deps: &Deps, args: &Value) -> Result<Value, String> {
    let account = s(args, "account");
    let id: u64 = s(args, "id").parse().map_err(|_| "id 格式不對".to_string())?;
    let raw = match deps.mail.request(account, Job::Peek { ids: vec![id] }).await? {
        Out::Raws(v) => v.into_iter().next().map(|(_, _, r)| r).ok_or("找不到這封信")?,
        _ => return Err("回應格式不對".into()),
    };
    Ok(mail_summary(&raw))
}

/// 一封信給模型看的內容
pub fn mail_summary(raw: &[u8]) -> Value {
    let Some(m) = mail_parser::MessageParser::default().parse(raw) else {
        return json!({ "error": "無法解析這封信" });
    };
    let addr = |l: Option<mail_parser::AddressList<'_>>| -> Vec<String> {
        l.map(|l| {
            l.mailboxes()
                .map(|x| match (x.name(), x.address()) {
                    (Some(n), Some(e)) if !n.is_empty() => format!("{n} <{e}>"),
                    (_, Some(e)) => e.to_string(),
                    (Some(n), None) => n.to_string(),
                    _ => String::new(),
                })
                .collect()
        })
        .unwrap_or_default()
    };
    let mut text = m
        .body_text(0)
        .map(|t| t.into_owned())
        .filter(|t| !t.trim().is_empty())
        .or_else(|| m.body_html(0).map(|h| mail_parser::html_to_text(&h)))
        .unwrap_or_default();
    let cut = text.chars().count() > MAX_BODY;
    if cut {
        text = text.chars().take(MAX_BODY).collect::<String>() + "\n…(後面省略)";
    }
    let attachments: Vec<String> = m
        .attachments()
        .filter_map(|a| a.attachment_name().map(str::to_string))
        .collect();
    json!({
        "subject": m.subject().unwrap_or(""),
        "from": addr(m.from()),
        "to": addr(m.to()),
        "cc": addr(m.cc()),
        "date": m.date().map(|d| local_time(d.to_timestamp())).unwrap_or_default(),
        "attachments": attachments,
        "body": text,
    })
}

fn todo_json(i: &todo::Item) -> Value {
    json!({
        "id": i.id, "title": i.title, "priority": todo::priority_name(i.priority), "done": i.done,
        "due": i.due, "notes": i.notes, "by": i.by,
    })
}

fn list_todos(deps: &Deps, args: &Value) -> Value {
    let all = args["include_done"].as_bool().unwrap_or(false);
    let items: Vec<Value> = deps.todo.list().iter().filter(|i| all || !i.done).map(todo_json).collect();
    json!({ "items": items })
}

fn add_todo(deps: &Deps, args: &Value) -> Result<Value, String> {
    let pri = args["priority"].as_u64().unwrap_or(2).clamp(1, 3) as u8;
    let due = Some(s(args, "due").to_string()).filter(|d| !d.is_empty());
    let it = deps.todo.add(s(args, "title"), pri, due, s(args, "notes"))?;
    Ok(json!({ "added": todo_json(&it) }))
}

fn update_todo(deps: &Deps, args: &Value) -> Result<Value, String> {
    let get = |k: &str| args[k].as_str().map(str::to_string);
    let patch = todo::Patch {
        title: get("title"),
        priority: args["priority"].as_u64().map(|p| p.clamp(1, 3) as u8),
        done: args["done"].as_bool(),
        notes: get("notes"),
        due: get("due"),
    };
    let it = deps.todo.update(s(args, "id"), patch)?;
    Ok(json!({ "updated": todo_json(&it) }))
}

fn invoice_status(deps: &Deps) -> Value {
    let views = deps.invoice.views();
    let count = |st: &str| views.iter().filter(|v| v.status == st).count();
    let wins: Vec<Value> = views
        .iter()
        .filter(|v| v.status == "won")
        .map(|v| {
            json!({
                "number": v.record.number, "period": v.record.period, "seller": v.record.seller,
                "prize": v.prize, "amount": v.prize_amount, "claim_from": v.claim_from, "claim_to": v.claim_to,
                "claimed": v.record.claimed, "auto_deposit": v.record.auto_deposit,
            })
        })
        .collect();
    json!({
        "total": views.len(),
        "pending": count("pending") + count("waiting"),
        "lost": count("lost"),
        "expired": count("expired"),
        "wins": wins,
        "unknown_mails": deps.invoice.unknown().len(),
    })
}

fn pct(used: u64, total: u64) -> Value {
    if total == 0 { Value::Null } else { json!((used as f64 * 1000.0 / total as f64).round() / 10.0) }
}

fn cluster_status(deps: &Deps) -> Value {
    let snap = deps.cluster.snapshot();
    let nodes: Vec<Value> = snap
        .nodes
        .iter()
        .map(|v| {
            let n = &v.node;
            let mut o = json!({
                "name": n.name, "role": n.role, "online": n.online && !v.stale, "this_machine": n.me,
                "last_seen": local_time(n.last_seen),
            });
            if let Some(i) = &n.info {
                o["os"] = json!(i.os);
                o["cpu_percent"] = json!((i.cpu_usage * 10.0).round() / 10.0);
                o["load"] = json!(i.load);
                o["memory_percent"] = pct(i.mem_used, i.mem_total);
                o["disk_percent"] = pct(i.disk_used, i.disk_total);
                o["disk_free"] = json!(cluster::human_bytes(i.disk_total.saturating_sub(i.disk_used)));
                o["temperature_c"] = json!(i.temp);
                o["uptime"] = json!(cluster::human_duration(i.uptime as i64));
                o["cng6_version"] = json!(i.version);
            }
            o
        })
        .collect();
    json!({
        "connected": snap.connected,
        "nodes": nodes,
        "active_alerts": snap.alerts_active.iter().map(|a| serde_json::to_value(a).unwrap_or_default()).collect::<Vec<_>>(),
    })
}

fn weather(deps: &Deps) -> Value {
    let cities: Vec<Value> = deps
        .weather
        .cities()
        .iter()
        .map(|c| {
            let mut o = json!({ "city": c.loc.name, "updated": c.updated, "error": c.error });
            if let Some(f) = &c.forecast {
                let cur = &f.current;
                o["now"] = json!({
                    "desc": weather::api::describe(cur.code), "temp_c": cur.temp, "feels_c": cur.feels,
                    "precip_mm": cur.precip, "wind_kmh": cur.wind,
                });
                o["days"] = f
                    .daily
                    .iter()
                    .take(5)
                    .map(|d| {
                        json!({
                            "date": d.date, "desc": weather::api::describe(d.code), "max_c": d.max, "min_c": d.min,
                            "rain_chance": d.prob, "precip_mm": d.precip,
                        })
                    })
                    .collect();
                o["alerts"] = serde_json::to_value(&f.alerts).unwrap_or_default();
            }
            o
        })
        .collect();
    json!({ "cities": cities })
}
