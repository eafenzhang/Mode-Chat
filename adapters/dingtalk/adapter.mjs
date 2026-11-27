// 钉钉 Stream 适配器（外挂适配器包 v1）。
//
//! 与宿主（ModeChat 应用）的 stdio JSONL 协议见 adapters/README.md：
//! - stdin  : {"op":"start","config":{...}} / {"op":"reply","context_token":...,"text":...}
//! - stdout : {"event":"status"|"message"|"log"|"ready", ...}
//!
//! 零 npm 依赖：Node 22+ 原生 WebSocket 与 fetch。

const GATEWAY_OPEN_URL = "https://api.dingtalk.com/v1.0/gateway/connections/open";
const TOPIC_MESSAGE = "/event/message";
const PING_INTERVAL_MS = 25_000;

let config = null;
let ws = null;
let pingTimer = null;
let aborted = false;
const seen = new Set();

function send(line) {
  process.stdout.write(JSON.stringify(line) + "\n");
}
const status = (s, detail = null) => send({ event: "status", status: s, detail });
const log = (line) => send({ event: "log", line });
const reqId = () => `req-${Date.now()}-${Math.random().toString(16).slice(2, 10)}`;

async function gatewayOpen() {
  const res = await fetch(GATEWAY_OPEN_URL, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      clientId: config.clientId,
      clientSecret: config.clientSecret,
      ua: "modechat-adapter/1",
      localIp: "127.0.0.1",
      subscriptions: [{ topic: TOPIC_MESSAGE, type: "EVENT", permissions: ["SUBSCRIPTION"] }],
    }),
  });
  if (!res.ok) throw new Error(`gateway HTTP ${res.status}`);
  const payload = await res.json();
  const endpoint = payload.endpoint || "";
  const ticket = payload.ticket || "";
  if (!endpoint || !ticket) throw new Error("gateway 响应缺 endpoint/ticket");
  return { endpoint, ticket };
}

async function connectLoop() {
  while (!aborted) {
    status("connecting");
    let endpoint, ticket;
    try {
      ({ endpoint, ticket } = await gatewayOpen());
    } catch (error) {
      status("reconnecting", String(error.message || error));
      await sleep(5_000);
      continue;
    }
    try {
      ws = new WebSocket(endpoint);
      await openOnce(ws);
      ws.send(JSON.stringify({
        code: ticket, frameType: "system", type: "REGISTER",
        contentType: "application/json", reqId: reqId(), appKey: config.clientId,
        ua: "modechat-adapter/1",
      }));
      if (!(await awaitAck(ws, "REGISTER:ACK"))) throw new Error("REGISTER:ACK 超时");
      status("connected");
      log("钉钉 Stream 已注册");
      await pump(ws);
    } catch (error) {
      status("reconnecting", String(error.message || error));
    }
    try { ws?.close(); } catch {}
    ws = null;
    if (!aborted) await sleep(3_000);
  }
}

function openOnce(socket) {
  return new Promise((resolve, reject) => {
    if (socket.readyState === 1) return resolve();
    socket.addEventListener("open", () => resolve(), { once: true });
    socket.addEventListener("error", (e) => reject(new Error(`WS 连接失败：${e.message}`)), { once: true });
  });
}

function awaitAck(socket, type) {
  return new Promise((resolve) => {
    const onMessage = (raw) => {
      try {
        const frame = JSON.parse(typeof raw === "string" ? raw : raw.data);
        if (frame.type === type) {
          socket.removeEventListener("message", onMessage);
          resolve(true);
        }
      } catch {}
    };
    socket.addEventListener("message", onMessage);
    setTimeout(() => {
      socket.removeEventListener("message", onMessage);
      resolve(false);
    }, 10_000);
  });
}

async function pump(socket) {
  let lastDownlink = Date.now();
  pingTimer = setInterval(() => {
    if (Date.now() - lastDownlink >= PING_INTERVAL_MS) {
      socket.send(JSON.stringify({ frameType: "system", type: "PING", contentType: "application/json", reqId: reqId() }));
    }
  }, PING_INTERVAL_MS);
  await new Promise((resolve) => {
    socket.addEventListener("message", (raw) => {
      lastDownlink = Date.now();
      let frame;
      try { frame = JSON.parse(typeof raw === "string" ? raw : raw.data); } catch { return; }
      switch (frame.type) {
        case "PING":
          socket.send(JSON.stringify({ frameType: "system", type: "PONG", contentType: "application/json" }));
          break;
        case "PING:ACK":
        case "REGISTER:ACK":
          break;
        case "EVENT": {
          socket.send(JSON.stringify({ frameType: "system", type: "EVENT-ACK", contentType: "application/json", reqId: frame.reqId, code: 200, message: "OK" }));
          const event = parseMessageEvent(frame);
          if (event) {
            if (event.messageId && seen.has(event.messageId)) break;
            if (event.messageId) seen.add(event.messageId);
            send({ event: "message", ...event });
          }
          break;
        }
        default:
          break;
      }
    });
    socket.addEventListener("close", () => resolve());
    socket.addEventListener("error", () => resolve());
  });
  clearInterval(pingTimer);
  pingTimer = null;
}

function parseMessageEvent(frame) {
  if (frame.headers?.topic !== TOPIC_MESSAGE) return null;
  let data = frame.data;
  if (typeof data === "string") {
    try { data = JSON.parse(data); } catch { return null; }
  }
  const text = (data?.text?.content || "").trim();
  if (!text) return null;
  return {
    messageId: data.msgId || "",
    senderId: data.senderStaffId || data.senderUnionId || "",
    senderNick: data.senderNick || null,
    chatType: data.conversationType === "1" ? "private" : "group",
    chatId: data.conversationId || "",
    text,
    contextToken: data.sessionWebhook || "",
  };
}

async function reply(webhook, text) {
  const res = await fetch(webhook, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ msgtype: "text", text: { content: text } }),
  });
  if (!res.ok) throw new Error(`钉钉 webhook 回复失败（HTTP ${res.status}）`);
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

process.stdin.setEncoding("utf8");
let buffer = "";
process.stdin.on("data", (chunk) => {
  buffer += chunk;
  let index;
  while ((index = buffer.indexOf("\n")) >= 0) {
    const line = buffer.slice(0, index).trim();
    buffer = buffer.slice(index + 1);
    if (!line) continue;
    let op;
    try { op = JSON.parse(line); } catch { continue; }
    if (op.op === "start") {
      config = op.config;
      if (!config?.clientId || !config?.clientSecret) {
        status("error", "钉钉 Client ID / Secret 未配置");
        continue;
      }
      seen.clear();
      aborted = false;
      connectLoop();
    } else if (op.op === "reply") {
      reply(op.context_token, op.text).catch((error) => log(`回复失败：${error.message || error}`));
    }
  }
});
process.stdin.on("end", () => {
  aborted = true;
  try { ws?.close(); } catch {}
  process.exit(0);
});
send({ event: "ready" });
