import init, { GroupChat } from '../pkg/slim_wasm.js';

// ─────────────────────────────────────────────
// State
// ─────────────────────────────────────────────

const me = {
  name: null,
  gc: null,
  inGroup: false,
  isCreator: false,
  members: [],
};

const peers = new Map();

let channel = null;
let heartbeatTimer = null;

const $ = (s) => document.querySelector(s);

// ─────────────────────────────────────────────
// Signaling protocol via BroadcastChannel
//
// Message types:
//   announce        — peer comes online
//   bye             — peer leaves
//   heartbeat       — keep-alive
//   group_created   — creator announces group exists
//   invite          — creator invites a peer (sends nothing, peer replies with key_package)
//   key_package     — invitee sends key package to creator
//   welcome         — creator sends welcome + commit to joiner
//   commit          — broadcast commit to existing members
//   encrypted_msg   — broadcast encrypted application message
//   rotation_proposal — member proposes credential rotation
//   rotation_commit — another member commits the rotation
// ─────────────────────────────────────────────

function broadcast(type, payload = {}) {
  channel.postMessage({ type, from: me.name, ts: Date.now(), ...payload });
}

function handleSignal(msg) {
  if (msg.from === me.name) return;

  switch (msg.type) {
    case 'announce':
    case 'heartbeat':
      registerPeer(msg.from);
      if (msg.type === 'announce') {
        broadcast('heartbeat');
        if (me.inGroup && me.isCreator) {
          broadcast('group_created', { creator: me.name, membersList: me.members });
        }
      }
      break;

    case 'bye':
      peers.delete(msg.from);
      refreshPeers();
      break;

    case 'group_created':
      registerPeer(msg.from);
      log(`${msg.creator} has a group (members: ${msg.membersList.join(', ')})`, 'info');
      if (!me.inGroup) {
        addSystemMsg(`${msg.creator} created a group — you can be invited`);
      }
      break;

    case 'invite':
      if (msg.to !== me.name) return;
      handleInvite(msg);
      break;

    case 'key_package':
      if (msg.to !== me.name) return;
      handleKeyPackage(msg);
      break;

    case 'welcome':
      if (msg.to !== me.name) return;
      handleWelcome(msg);
      break;

    case 'commit':
      if (!me.inGroup || msg.from === me.name) return;
      handleCommit(msg);
      break;

    case 'encrypted_msg':
      if (!me.inGroup || msg.from === me.name) return;
      handleEncryptedMsg(msg);
      break;

    case 'rotation_proposal':
      if (!me.inGroup || msg.from === me.name) return;
      handleRotationProposal(msg);
      break;

    case 'rotation_commit':
      if (!me.inGroup || msg.from === me.name) return;
      handleRotationCommit(msg);
      break;
  }
}

// ─────────────────────────────────────────────
// Peer tracking
// ─────────────────────────────────────────────

function registerPeer(name) {
  peers.set(name, Date.now());
  refreshPeers();
}

function refreshPeers() {
  const bar = $('#peers-bar');
  bar.innerHTML = '';
  for (const [name] of peers) {
    const d = document.createElement('span');
    d.className = 'peer-dot';
    d.textContent = name;
    bar.appendChild(d);
  }
  refreshInviteSelect();
}

function refreshInviteSelect() {
  const sel = $('#invite-select');
  if (!sel) return;
  sel.innerHTML = '';
  for (const [name] of peers) {
    if (me.members.includes(name)) continue;
    const opt = document.createElement('option');
    opt.value = name;
    opt.textContent = name;
    sel.appendChild(opt);
  }
}

// ─────────────────────────────────────────────
// Group lifecycle
// ─────────────────────────────────────────────

async function createGroup() {
  try {
    const gid = await me.gc.createGroup();
    me.inGroup = true;
    me.isCreator = true;
    me.members = [me.name];
    log(`Group created (${toHex(gid).slice(0, 16)}…)`, 'ok');
    addSystemMsg('You created a new group');
    broadcast('group_created', { creator: me.name, membersList: me.members });
    refreshUI();
  } catch (e) {
    log(`Create group failed: ${e}`, 'err');
  }
}

async function invitePeer() {
  const target = $('#invite-select').value;
  if (!target) return;
  log(`Sending invite to ${target}…`, 'info');
  broadcast('invite', { to: target });
}

async function handleInvite(msg) {
  if (me.inGroup) {
    log(`Ignoring invite from ${msg.from} — already in a group`, 'warn');
    return;
  }
  try {
    log(`Invite from ${msg.from} — generating key package…`, 'info');
    addSystemMsg(`${msg.from} invited you — joining…`);
    const kp = await me.gc.generateKeyPackage();
    broadcast('key_package', { to: msg.from, keyPackage: Array.from(kp) });
    log(`Sent key package to ${msg.from} (${kp.length}B)`, 'ok');
  } catch (e) {
    log(`Key package generation failed: ${e}`, 'err');
  }
}

async function handleKeyPackage(msg) {
  if (!me.isCreator) return;
  try {
    const kp = new Uint8Array(msg.keyPackage);
    log(`Received key package from ${msg.from} (${kp.length}B) — adding member…`, 'info');
    const result = await me.gc.addMember(kp);

    for (const memberName of me.members) {
      if (memberName === me.name) continue;
      broadcast('commit', { commitMessage: Array.from(result.commitMessage) });
    }

    me.members.push(msg.from);

    broadcast('welcome', {
      to: msg.from,
      welcomeMessage: Array.from(result.welcomeMessage),
      membersList: [...me.members],
    });

    log(`${msg.from} added to group`, 'ok');
    addSystemMsg(`${msg.from} joined the group`);
    refreshUI();
  } catch (e) {
    log(`Add member failed: ${e}`, 'err');
  }
}

async function handleWelcome(msg) {
  try {
    const welcome = new Uint8Array(msg.welcomeMessage);
    log(`Received welcome message (${welcome.length}B) — joining group…`, 'info');
    const gid = await me.gc.joinGroup(welcome);
    me.inGroup = true;
    me.isCreator = false;
    me.members = [...msg.membersList];
    log(`Joined group (${toHex(gid).slice(0, 16)}…)`, 'ok');
    addSystemMsg(`You joined the group (members: ${me.members.join(', ')})`);
    refreshUI();
  } catch (e) {
    log(`Join group failed: ${e}`, 'err');
  }
}

async function handleCommit(msg) {
  try {
    const commit = new Uint8Array(msg.commitMessage);
    await me.gc.processCommit(commit);
    if (msg.newMember) {
      me.members.push(msg.newMember);
      addSystemMsg(`${msg.newMember} joined the group`);
    }
    if (msg.membersList) {
      me.members = [...msg.membersList];
    }
    log(`Processed commit from ${msg.from}`, 'ok');
    refreshUI();
  } catch (e) {
    log(`Process commit failed: ${e}`, 'err');
  }
}

// ─────────────────────────────────────────────
// Messaging
// ─────────────────────────────────────────────

async function sendMessage() {
  const input = $('#chat-input');
  const text = input.value.trim();
  if (!text || !me.inGroup) return;

  try {
    const plaintext = new TextEncoder().encode(text);
    const ciphertext = await me.gc.encrypt(plaintext);
    log(`Encrypted ${plaintext.length}B → ${ciphertext.length}B`, 'info');

    addOutgoingMsg(text, ciphertext.length);
    broadcast('encrypted_msg', { ciphertext: Array.from(ciphertext), senderName: me.name });
    input.value = '';
  } catch (e) {
    log(`Encrypt failed: ${e}`, 'err');
  }
}

async function handleEncryptedMsg(msg) {
  try {
    const ct = new Uint8Array(msg.ciphertext);
    const decrypted = await me.gc.decrypt(ct);
    const text = new TextDecoder().decode(decrypted);
    log(`Decrypted message from ${msg.senderName}: "${text}"`, 'ok');
    addIncomingMsg(msg.senderName, text, ct.length);
  } catch (e) {
    log(`Decrypt failed: ${e}`, 'err');
  }
}

// ─────────────────────────────────────────────
// Credential rotation
// ─────────────────────────────────────────────

async function rotateCredentials() {
  if (!me.inGroup) return;
  try {
    const proposal = await me.gc.rotateCredentials();
    log(`Rotation proposal created (${proposal.length}B)`, 'info');
    broadcast('rotation_proposal', { proposal: Array.from(proposal), proposer: me.name });
    addSystemMsg('You proposed a credential rotation');
  } catch (e) {
    log(`Rotation proposal failed: ${e}`, 'err');
  }
}

async function handleRotationProposal(msg) {
  try {
    const proposal = new Uint8Array(msg.proposal);
    log(`Rotation proposal from ${msg.proposer} — committing…`, 'info');
    const commit = await me.gc.processProposal(proposal, true);
    broadcast('rotation_commit', { commitMessage: Array.from(commit), proposer: msg.proposer, committer: me.name });
    addSystemMsg(`${msg.proposer} rotated credentials (you committed)`);
    log(`Rotation committed (${commit.length}B)`, 'ok');
    refreshUI();
  } catch (e) {
    log(`Process rotation proposal failed: ${e}`, 'err');
  }
}

async function handleRotationCommit(msg) {
  try {
    const commit = new Uint8Array(msg.commitMessage);
    if (msg.committer === me.name) return;
    await me.gc.processCommit(commit);
    addSystemMsg(`${msg.proposer} rotated credentials (committed by ${msg.committer})`);
    log(`Processed rotation commit`, 'ok');
    refreshUI();
  } catch (e) {
    log(`Process rotation commit failed: ${e}`, 'err');
  }
}

// ─────────────────────────────────────────────
// UI helpers
// ─────────────────────────────────────────────

function refreshUI() {
  const gc = me.gc;
  const gid = gc?.groupId;
  const epoch = gc?.epoch;

  if (gid) {
    $('#g-status').textContent = me.isCreator ? 'Creator' : 'Member';
    $('#g-status').className = 'val active';
    $('#g-id').textContent = toHex(gid).slice(0, 24) + '…';
    $('#g-epoch').textContent = epoch ?? '—';
    $('#btn-create-group').disabled = true;
    $('#chat-input-bar').style.display = '';
  }

  if (me.inGroup) {
    $('#members-section').style.display = '';
    const list = $('#members-list');
    list.innerHTML = '';
    for (const m of me.members) {
      const li = document.createElement('li');
      li.textContent = m;
      if (m === me.name) li.className = 'me';
      list.appendChild(li);
    }

    if (me.isCreator) {
      $('#invite-section').style.display = '';
      refreshInviteSelect();
    }

    $('#advanced-section').style.display = me.members.length >= 2 ? '' : 'none';
  }
}

function addOutgoingMsg(text, ctLen) {
  const el = document.createElement('div');
  el.className = 'msg outgoing';
  el.innerHTML =
    `<div class="msg-sender">${esc(me.name)}</div>` +
    `<div>${esc(text)}</div>` +
    `<div class="msg-meta">encrypted: ${ctLen}B</div>`;
  appendChat(el);
}

function addIncomingMsg(sender, text, ctLen) {
  const el = document.createElement('div');
  el.className = 'msg incoming';
  el.innerHTML =
    `<div class="msg-sender">${esc(sender)}</div>` +
    `<div>${esc(text)}</div>` +
    `<div class="msg-meta">decrypted from ${ctLen}B ciphertext</div>`;
  appendChat(el);
}

function addSystemMsg(text) {
  const el = document.createElement('div');
  el.className = 'msg system';
  el.textContent = text;
  appendChat(el);
}

function appendChat(el) {
  const c = $('#chat-messages');
  c.appendChild(el);
  c.scrollTop = c.scrollHeight;
}

function log(msg, level = 'info') {
  const el = $('#event-log');
  const t = new Date().toLocaleTimeString('en-US', { hour12: false, fractionalSecondDigits: 2 });
  const d = document.createElement('div');
  d.className = `log-entry ${level}`;
  d.innerHTML = `<span class="ts">${t}</span>${esc(msg)}`;
  el.prepend(d);
}

function esc(s) {
  const d = document.createElement('span');
  d.textContent = s;
  return d.innerHTML;
}

function toHex(u8) {
  return Array.from(u8).map(b => b.toString(16).padStart(2, '0')).join('');
}

// ─────────────────────────────────────────────
// Boot
// ─────────────────────────────────────────────

async function joinAs(name, secret) {
  log('Loading WASM…');
  await init();
  log('WASM loaded', 'ok');

  me.gc = await new GroupChat(name, secret);
  me.name = name;
  log(`Initialized MLS client as "${name}"`, 'ok');

  channel = new BroadcastChannel('slim-mls-validation');
  channel.onmessage = (e) => handleSignal(e.data);

  broadcast('announce');
  heartbeatTimer = setInterval(() => broadcast('heartbeat'), 3000);
  window.addEventListener('beforeunload', () => {
    broadcast('bye');
    clearInterval(heartbeatTimer);
  });

  // Prune stale peers
  setInterval(() => {
    const now = Date.now();
    for (const [name, ts] of peers) {
      if (now - ts > 10000) {
        peers.delete(name);
        refreshPeers();
      }
    }
  }, 5000);

  $('#login-screen').style.display = 'none';
  $('#app-screen').style.display = '';
  $('#my-identity').textContent = name;
  document.title = `SLIM MLS — ${name}`;
}

// ── Event listeners ──

$('#btn-join').addEventListener('click', () => {
  const name = $('#input-name').value.trim().toLowerCase();
  const secret = $('#input-secret').value;
  if (!name) return;
  joinAs(name, secret);
});

$('#input-name').addEventListener('keydown', (e) => {
  if (e.key === 'Enter') $('#btn-join').click();
});

$('#btn-create-group').addEventListener('click', createGroup);
$('#btn-invite').addEventListener('click', invitePeer);
$('#btn-send').addEventListener('click', sendMessage);
$('#chat-input').addEventListener('keydown', (e) => {
  if (e.key === 'Enter') sendMessage();
});
$('#btn-rotate').addEventListener('click', rotateCredentials);
