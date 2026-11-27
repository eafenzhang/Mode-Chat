// 飞书长连接适配器（外挂适配器包 v1）。
//
//! 帧协议按官方 SDK（oapi-sdk-python / oapi-sdk-go 的 ws 实现）实现：
//! - endpoint：POST {domain}/event/v1/outbound_ws/endpoint，body {appID, appSecret, clientID?, connections:[{serviceID}]}
//!   → {code, data:{URL, ticket}}
//! - WS 帧：二进制。header 4 字节：[0]=protocol version(0)、[1]=frame type、[2..4] 保留；
//!   随后 4 字节 payload 长度（BE u32）+ payload（JSON UTF-8）。
//!   frame type：2=SIGNAL(JSON2)、3=DATA(JSON3)、4=ERROR、5=CONTROL。
//! - JSON2 语义：客户端 {"type":"ping"} → 服务端 {"type":"pong"}；
//!   连接注册 {"type":"event_connect","headers":{"auth":"bearer <app_access_token>","service_id":1},...}
//! - JSON3 语义：下行事件 {"type":"event","headers":{...event_id/event_type...},"body":{...}}，
//!   客户端回 {"type":"event_reply","headers":{...同 event_id...},"body":{"code":200}}
//! 协议细节若有出入，由适配器包在线更新修正（本文件不随应用发版）。
//!
//! 零 npm 依赖：Node 22+ 原生 WebSocket 与 fetch。

const DOMAIN = "https://open.feishu.cn";
const TOKEN_URL = `${DOMAIN}/open-apis/auth/v3/app_access_token/internal`;
const ENDPOINT_URL = `${DOMAIN}/open-apis/event/v1/outbound_ws/endpoint`;
const SERVICE_ID = 1;
const PING_INTERVAL_MS = 25_000;

const FRAME_TYPE_SIGNAL = 2;
const FRAME_TYPE_DATA = 3;
const FRAME_TYPE_ERROR = 4;
const FRAME_TYPE_CONTROL = 5;

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

async function appAccessToken() {
  const res = await fetch(TOKEN_URL, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ app_id: config.appId, app_secret: config.appSecret }),
  });
  const payload = await res.json();
  const token = payload.app_access_token || payload.data?.app_access_token;
  if (!token) throw new Error(`拿 app_access_token 失败：${payload.msg || res.status}`);
  return token;
}

async function endpointUrl(token) {
  const res = await fetch(ENDPOINT_URL, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${token}`,
    },
    body: JSON.stringify({
      appID: config.appId,
      appSecret: config.appSecret,
      connections: [{ serviceID: SERVICE_ID }],
    }),
  });
  const payload = await res.json();
  const url = payload.data?.URL || payload.data?.url || payload.URL || "";
  const ticket = payload.data?.ticket || payload.ticket || "";
  if (!url) throw new Error(`endpoint 响应缺 URL：${payload.msg || res.status}`);
  return { url, ticket };
}

function encodeFrame(frameType, payloadObj) {
  const payload = Buffer.from(JSON.stringify(payloadObj), "utf8");
  const header = Buffer.alloc(8);
  header[0] = 0; // protocol version
  header[1] = frameType;
  header.writeUInt32BE(payload.length, 4);
  return Buffer.concat([header, payload]);
}

function decodeFrame(buffer) {
  if (buffer.length < 8) return null;
  const frameType = buffer[1];
  const length = buffer.readUInt32BE(4);
  const payload = buffer.subarray(8, 8 + length);
  let json = null;
  try { json = JSON.parse(payload.toString("utf8")); } catch {}
  return { frameType, json };
}

async function connectLoop() {
  while (!aborted) {
    status("connecting");
    let token, url;
    try {
      token = await appAccessToken();
      ({ url } = await endpointUrl(token));
    } catch (error) {
      status("reconnecting", String(error.message || error));
      await sleep(5_000);
      continue;
    }
    try {
      ws = new WebSocket(url);
      await openOnce(ws);
      // 注册（JSON2 signal）：auth 用 bearer app_access_token。
      ws.send(encodeFrame(FRAME_TYPE_SIGNAL, {
        type: "event_connect",
        headers: { auth: `bearer ${token}`, service_id: SERVICE_ID },
        payload: { ticket: config.lastTicket || "" },
        reqId: reqId(),
      }));
      status("connected");
      log("飞书长连接已发起");
      await pump(ws, token);
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

async function handleBinary(data) {
  const bytes = await await_bytes(data);
  const frame = decodeFrame(Buffer.from(bytes));
  if (!frame) return;
  switch (frame.frameType) {
    case FRAME_TYPE_SIGNAL: {
      // 服务端 ping → pong。
      if (frame.json?.type === "ping") {
        ws.send(encodeFrame(FRAME_TYPE_SIGNAL, { type: "pong", reqId: frame.json.reqId || reqId() }));
      }
      break;
    }
    case FRAME_TYPE_DATA: {
      const headers = frame.json?.headers || {};
      const eventId = headers.event_id || "";
      // 回执（语义按官方 SDK：type=event_reply + 同 event_id）。
      ws.send(encodeFrame(FRAME_TYPE_DATA, {
        type: "event_reply",
        headers: { event_id: eventId, service_id: headers.service_id || SERVICE_ID },
        body: { code: 200 },
        reqId: reqId(),
      }));
      const body = frame.json?.body || {};
      const text = extractText(body);
      if (!text) break;
      if (eventId && seen.has(eventId)) break;
      if (eventId) seen.add(eventId);
      send({
        event: "message",
        messageId: eventId,
        senderId: body.sender_id?.open_id || body.sender_id?.user_id || "",
        senderNick: body.sender_id?.sender_nick || body.sender_nick || null,
        chatType: body.message?.chat_type === "group" ? "group" : "private",
        chatId: body.message?.chat_id || "",
        text,
        contextToken: JSON.stringify({ chatId: body.message?.chat_id || "", messageId: body.message?.message_id || "" }),
      });
      break;
    }
    case FRAME_TYPE_CONTROL:
      // 服务端控制帧（重连指示等）：直接断开走重连。
      throw new Error(`收到控制帧，重连`);
    case FRAME_TYPE_ERROR:
      log(`飞书错误帧：${JSON.stringify(frame.json)}`);
      break;
    default:
      break;
  }
}

function extractText(body) {
  // im.message.content_v1（JSON 字符串）与 content 字段双兼容。
  let content = body.message?.content || "";
  if (typeof content === "string" && content.startsWith("{")) {
    try {
      const parsed = JSON.parse(content);
      content = parsed.text || content;
    } catch {}
  }
  const text = String(content || "").trim();
  return text;
}

async function pump(socket, token) {
  pingTimer = setInterval(() => {
    socket.send(encodeFrame(FRAME_TYPE_SIGNAL, { type: "ping", reqId: reqId() }));
  }, PING_INTERVAL_MS);
  await new Promise((resolve) => {
    socket.addEventListener("message", (raw) => {
      try { handleBinary(raw.data); } catch (error) {
        log(`帧处理失败：${error.message || error}`);
        resolve();
      }
    });
    socket.addEventListener("close", () => resolve());
    socket.addEventListener("error", () => resolve());
  });
  clearInterval(pingTimer);
  pingTimer = null;
}

async function await_bytes(data) {
  if (data instanceof ArrayBuffer) return new Uint8Array(data);
  if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  if (data instanceof Blob) return new Uint8Array(await data.arrayBuffer());
  return new TextEncoder().encode(String(data));
}

// 飞书回复走 OpenAPI 发消息（im/v1/messages）；首版文本回复。
async function sendMessage(chatId, msgType, content) {
  const token = await appAccessToken();
  if (!chatId) throw new Error("缺少 chatId，无法回复");
  const res = await fetch(`${DOMAIN}/open-apis/im/v1/messages?receive_id_type=chat_id`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${token}`,
    },
    body: JSON.stringify({
      receive_id: chatId,
      msg_type: msgType,
      content: JSON.stringify(content),
    }),
  });
  const payload = await res.json();
  if ((payload.code ?? 0) !== 0) throw new Error(`飞书发消息失败：${payload.msg || res.status}`);
}

async function reply(contextToken, text) {
  let ctx = {};
  try { ctx = JSON.parse(contextToken || "{}"); } catch {}
  await sendMessage(ctx.chatId || "", "text", { text });
}

// ---------- 发文件 / 图片（OpenAPI 媒体上传 → 消息） ----------

async function uploadMedia(path, isImage) {
  const fs = await import("node:fs/promises");
  const bytes = await fs.readFile(path);
  const filename = path.split(/[\\/]/).pop() || (isImage ? "image.png" : "file");
  const token = await appAccessToken();
  const form = new FormData();
  if (isImage) {
    form.append("image_type", "message");
    form.append("image", new Blob([bytes]), filename);
  } else {
    form.append("file_type", "stream");
    form.append("file_name", filename);
    form.append("file", new Blob([bytes]), filename);
  }
  const url = isImage
    ? `${DOMAIN}/open-apis/im/v1/images`
    : `${DOMAIN}/open-apis/im/v1/files`;
  const res = await fetch(url, {
    method: "POST",
    headers: { Authorization: `Bearer ${token}` },
    body: form,
  });
  const payload = await res.json();
  const data = payload.data || {};
  const key = isImage ? data.image_key : data.file_key;
  if ((payload.code ?? 0) !== 0 || !key) {
    throw new Error(`飞书媒体上传失败：${payload.msg || res.status}`);
  }
  return key;
}

async function handleReply(op) {
  let ctx = {};
  try { ctx = JSON.parse(op.context_token || "{}"); } catch {}
  if (op.media_path) {
    const isImage = /\.(png|jpe?g|gif|bmp|webp)$/i.test(op.media_path);
    const key = await uploadMedia(op.media_path, isImage);
    if (isImage) {
      await sendMessage(ctx.chatId || "", "image", { image_key: key });
    } else {
      await sendMessage(ctx.chatId || "", "file", { file_key: key });
    }
    if (op.caption) await reply(op.context_token, op.caption);
    return;
  }
  const text = (op.text || "").trim();
  if (!text) return;
  await reply(op.context_token, text);
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
      if (!config?.appId || !config?.appSecret) {
        status("error", "飞书 App ID / App Secret 未配置");
        continue;
      }
      seen.clear();
      aborted = false;
      connectLoop();
    } else if (op.op === "reply") {
      handleReply(op).catch((error) => log(`回复失败：${error.message || error}`));
    }
  }
});
process.stdin.on("end", () => {
  aborted = true;
  try { ws?.close(); } catch {}
  process.exit(0);
});
send({ event: "ready" });
