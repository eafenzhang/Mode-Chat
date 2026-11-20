// 最小 ACP 服务：用于验证图里的审批卡（request_permission）链路。
import readline from 'node:readline';
const rl = readline.createInterface({ input: process.stdin });
let pendingPrompt = null;
const send = (obj) => process.stdout.write(JSON.stringify(obj) + '\n');
const chunk = (text) => send({ jsonrpc:'2.0', method:'session/update', params:{ sessionId:'mock-session-1', update:{ sessionUpdate:'agent_message_chunk', content:{ type:'text', text } } } });
rl.on('line', (line) => {
  let msg; try { msg = JSON.parse(line); } catch { return; }
  if (msg.method === 'initialize') {
    send({ jsonrpc:'2.0', id:msg.id, result:{ protocolVersion:1, agentInfo:{ name:'mock-acp', version:'0.1.0' }, agentCapabilities:{ loadSession:true } } });
  } else if (msg.method === 'session/new') {
    send({ jsonrpc:'2.0', id:msg.id, result:{ sessionId:'mock-session-1' } });
    chunk('（mock）会话已建立。\n');
  } else if (msg.method === 'session/load') {
    // 模拟 pi-acp：历史帧在 load 响应之前重放。客户端必须在 ready 前丢弃它们。
    chunk('（历史）记住 4242。\n');
    send({ jsonrpc:'2.0', id:msg.id, result:{} });
  } else if (msg.method === 'session/prompt') {
    pendingPrompt = msg.id;
    const text = msg.params?.prompt?.[0]?.text ?? '';
    if (text.includes('elicit')) {
      chunk('请填写表单。\n');
      send({ jsonrpc:'2.0', id:'elicit-200', method:'elicitation/create', params:{
        sessionId:'mock-session-1',
        mode:'form',
        message:'请选择并填写备注',
        requestedSchema:{ type:'object', properties:{
          q_0:{ type:'string', title:'选一个', oneOf:[
            { const:'A', title:'A 选项' },
            { const:'B', title:'B 选项' }
          ]},
          tags:{ type:'array', title:'多选', items:{ anyOf:[
            { const:'x', title:'X' }, { const:'y', title:'Y' }
          ]}},
          note:{ type:'string', title:'备注' }
        }, required:['q_0'] }
      }});
      return;
    }
    chunk('准备执行一条危险命令，先请你确认。\n');
    send({ jsonrpc:'2.0', id:100, method:'session/request_permission', params:{
      sessionId:'mock-session-1',
      toolCall:{ toolCallId:'call-1', title:'rm -rf /tmp/v4rm', kind:'execute' },
      options:[
        { optionId:'allow', name:'允许一次', kind:'allow_once' },
        { optionId:'deny', name:'拒绝', kind:'reject_once' }
      ]
    }});
  } else if (msg.id === 100 && msg.result) {
    const option = msg.result?.outcome?.optionId;
    chunk(option === 'allow' ? '已允许，命令执行完成。\n' : '已拒绝，命令没有执行。\n');
    if (pendingPrompt !== null) {
      send({ jsonrpc:'2.0', id:pendingPrompt, result:{ stopReason:'end_turn' } });
      pendingPrompt = null;
    }
  } else if (msg.id === 'elicit-200' && msg.result) {
    const action = msg.result?.action;
    const content = msg.result?.content ?? null;
    chunk(`表单回执：${action} ${JSON.stringify(content)}\n`);
    if (pendingPrompt !== null) {
      send({ jsonrpc:'2.0', id:pendingPrompt, result:{ stopReason:'end_turn' } });
      pendingPrompt = null;
    }
  }
});
