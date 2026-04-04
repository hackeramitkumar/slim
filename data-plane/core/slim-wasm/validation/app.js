import init, { GroupChat } from '../pkg/slim_wasm.js';

const state = {
  participants: new Map(),
  groupMembers: [],
  groupCreator: null,
};

function $(sel) { return document.querySelector(sel); }

function logEvent(msg, level = 'info') {
  const el = $('#event-log');
  const t = new Date().toLocaleTimeString('en-US', { hour12: false, fractionalSecondDigits: 3 });
  const div = document.createElement('div');
  div.className = `log-entry ${level}`;
  div.innerHTML = `<span class="time">${t}</span>${escapeHtml(msg)}`;
  el.prepend(div);
}

function escapeHtml(s) {
  const d = document.createElement('div');
  d.textContent = s;
  return d.innerHTML;
}

function toHex(u8) {
  return Array.from(u8).map(b => b.toString(16).padStart(2, '0')).join('');
}

function refreshParticipantChips() {
  const list = $('#participants-list');
  list.innerHTML = '';
  for (const [name] of state.participants) {
    const inGroup = state.groupMembers.includes(name);
    const isCreator = state.groupCreator === name;
    const chip = document.createElement('span');
    chip.className = `chip${inGroup ? ' in-group' : ''}${isCreator ? ' creator' : ''}`;
    chip.innerHTML = `<span class="dot"></span>${escapeHtml(name)}` +
      (isCreator ? ' <small>(creator)</small>' : '') +
      (inGroup && !isCreator ? ' <small>(member)</small>' : '') +
      ` <button class="remove-btn" data-name="${escapeHtml(name)}">&times;</button>`;
    list.appendChild(chip);
  }
  list.querySelectorAll('.remove-btn').forEach(btn => {
    btn.addEventListener('click', () => removeParticipant(btn.dataset.name));
  });
}

function refreshSelects() {
  const names = [...state.participants.keys()];
  const notInGroup = names.filter(n => !state.groupMembers.includes(n));
  const inGroup = names.filter(n => state.groupMembers.includes(n));

  fillSelect('#group-creator', names);
  fillSelect('#member-to-add', notInGroup);
  fillSelect('#msg-sender', inGroup);
  fillSelect('#rotate-who', inGroup);
  fillSelect('#rotate-committer', inGroup);

  const hasParticipants = names.length > 0;
  $('#group-panel').style.display = hasParticipants ? '' : 'none';
}

function fillSelect(sel, options) {
  const el = $(sel);
  const prev = el.value;
  el.innerHTML = '';
  for (const o of options) {
    const opt = document.createElement('option');
    opt.value = o;
    opt.textContent = o;
    el.appendChild(opt);
  }
  if (options.includes(prev)) el.value = prev;
}

function refreshGroupInfo() {
  const creator = state.participants.get(state.groupCreator);
  if (!creator) return;

  const gid = creator.groupId;
  const epoch = creator.epoch;

  if (gid) {
    $('#group-info').style.display = '';
    $('#group-id-display').textContent = toHex(gid).slice(0, 32) + '…';
    $('#epoch-display').textContent = epoch ?? '—';
    $('#members-display').textContent = state.groupMembers.join(', ') || '—';
    $('#add-member-section').style.display = '';
    $('#messaging-panel').style.display = '';
    $('#rotation-section').style.display = state.groupMembers.length >= 2 ? '' : 'none';
  }
  refreshSelects();
  refreshParticipantChips();
}

async function addParticipant() {
  const nameInput = $('#participant-name');
  const name = nameInput.value.trim().toLowerCase();
  if (!name) return;
  if (state.participants.has(name)) {
    logEvent(`Participant "${name}" already exists`, 'warn');
    return;
  }

  const secret = $('#shared-secret').value;
  try {
    const gc = await new GroupChat(name, secret);
    state.participants.set(name, gc);
    logEvent(`Added participant: ${name}`, 'success');
    nameInput.value = '';
    refreshSelects();
    refreshParticipantChips();
  } catch (e) {
    logEvent(`Failed to create participant "${name}": ${e}`, 'error');
  }
}

function removeParticipant(name) {
  const gc = state.participants.get(name);
  if (gc) gc.free();
  state.participants.delete(name);
  state.groupMembers = state.groupMembers.filter(n => n !== name);
  if (state.groupCreator === name) {
    state.groupCreator = null;
    $('#group-info').style.display = 'none';
    $('#add-member-section').style.display = 'none';
    $('#messaging-panel').style.display = 'none';
    $('#rotation-section').style.display = 'none';
  }
  refreshSelects();
  refreshParticipantChips();
  logEvent(`Removed participant: ${name}`, 'info');
}

async function createGroup() {
  const name = $('#group-creator').value;
  if (!name) return;
  const gc = state.participants.get(name);
  try {
    const gid = await gc.createGroup();
    state.groupCreator = name;
    state.groupMembers = [name];
    logEvent(`Group created by ${name} (id: ${toHex(gid).slice(0, 16)}…)`, 'success');
    addChatSystem(`${name} created the group`);
    refreshGroupInfo();
  } catch (e) {
    logEvent(`Create group failed: ${e}`, 'error');
  }
}

async function addMember() {
  const name = $('#member-to-add').value;
  if (!name || !state.groupCreator) return;

  const creator = state.participants.get(state.groupCreator);
  const joiner = state.participants.get(name);

  try {
    const kp = await joiner.generateKeyPackage();
    logEvent(`Key package generated for ${name} (${kp.length} bytes)`, 'info');

    const result = await creator.addMember(kp);
    logEvent(`${state.groupCreator} added ${name} — welcome: ${result.welcomeMessage.length}B, commit: ${result.commitMessage.length}B`, 'info');

    for (const memberName of state.groupMembers) {
      if (memberName === state.groupCreator) continue;
      const m = state.participants.get(memberName);
      await m.processCommit(result.commitMessage);
      logEvent(`${memberName} processed commit`, 'info');
    }

    await joiner.joinGroup(result.welcomeMessage);
    state.groupMembers.push(name);
    logEvent(`${name} joined the group`, 'success');
    addChatSystem(`${name} joined the group`);
    refreshGroupInfo();
  } catch (e) {
    logEvent(`Add member failed: ${e}`, 'error');
  }
}

async function sendMessage() {
  const sender = $('#msg-sender').value;
  const text = $('#msg-text').value.trim();
  if (!sender || !text) return;

  const senderGc = state.participants.get(sender);
  const plaintext = new TextEncoder().encode(text);

  try {
    const ciphertext = await senderGc.encrypt(plaintext);
    logEvent(`${sender} encrypted ${plaintext.length}B → ${ciphertext.length}B`, 'info');

    addChatMsg(sender, text, `encrypted: ${ciphertext.length}B`);

    for (const memberName of state.groupMembers) {
      if (memberName === sender) continue;
      const m = state.participants.get(memberName);
      const decrypted = await m.decrypt(ciphertext);
      const decText = new TextDecoder().decode(decrypted);
      logEvent(`${memberName} decrypted: "${decText}"`, 'success');
      addChatMsg(memberName, `↳ decrypted: "${decText}"`, `from ${sender}`);
    }

    $('#msg-text').value = '';
  } catch (e) {
    logEvent(`Send message failed: ${e}`, 'error');
  }
}

async function rotateCredentials() {
  const who = $('#rotate-who').value;
  const committer = $('#rotate-committer').value;
  if (!who || !committer || who === committer) {
    logEvent('Rotation requires two different group members', 'warn');
    return;
  }

  const whoGc = state.participants.get(who);
  const committerGc = state.participants.get(committer);

  try {
    const proposal = await whoGc.rotateCredentials();
    logEvent(`${who} created rotation proposal (${proposal.length}B)`, 'info');

    const commit = await committerGc.processProposal(proposal, true);
    logEvent(`${committer} committed rotation (${commit.length}B)`, 'info');

    for (const memberName of state.groupMembers) {
      if (memberName === committer) continue;
      const m = state.participants.get(memberName);
      await m.processCommit(commit);
      logEvent(`${memberName} processed rotation commit`, 'info');
    }

    logEvent(`Credential rotation complete for ${who}`, 'success');
    addChatSystem(`${who} rotated credentials (committed by ${committer})`);
    refreshGroupInfo();
  } catch (e) {
    logEvent(`Rotation failed: ${e}`, 'error');
  }
}

function addChatMsg(sender, text, meta) {
  const log = $('#chat-log');
  const div = document.createElement('div');
  div.className = 'chat-msg';
  div.innerHTML = `<span class="sender">${escapeHtml(sender)}</span>${escapeHtml(text)}` +
    (meta ? `<div class="meta">${escapeHtml(meta)}</div>` : '');
  log.appendChild(div);
  log.scrollTop = log.scrollHeight;
}

function addChatSystem(text) {
  const log = $('#chat-log');
  const div = document.createElement('div');
  div.className = 'chat-msg system';
  div.textContent = text;
  log.appendChild(div);
  log.scrollTop = log.scrollHeight;
}

// ─── Automated Tests ───

async function runTests() {
  const SECRET = $('#shared-secret').value;
  const results = $('#test-results');
  const summary = $('#test-summary');
  results.innerHTML = '';
  summary.textContent = '';
  summary.className = '';

  let passed = 0, failed = 0;

  function tlog(msg, cls) {
    const div = document.createElement('div');
    div.className = `test-line ${cls}`;
    div.textContent = msg;
    results.appendChild(div);
  }

  async function test(name, fn) {
    tlog(`▶ ${name} …`, 'running');
    try {
      await fn();
      tlog(`  ✓ ${name}`, 'pass');
      passed++;
    } catch (e) {
      tlog(`  ✗ ${name}: ${e.message || e}`, 'fail');
      console.error(`FAILED: ${name}`, e);
      failed++;
    }
  }

  function assert(c, m) { if (!c) throw new Error(m); }
  function assertEq(a, b, m) { if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${m}: ${JSON.stringify(a)} !== ${JSON.stringify(b)}`); }
  function assertBytes(a, b, m) {
    const x = Array.from(a instanceof Uint8Array ? a : new Uint8Array(a));
    const y = Array.from(b instanceof Uint8Array ? b : new Uint8Array(b));
    if (x.length !== y.length || !x.every((v,i) => v === y[i])) throw new Error(`${m}: bytes differ`);
  }

  await test('Initialization', async () => {
    const gc = await new GroupChat('t_alice', SECRET);
    assert(gc.groupId === undefined, 'groupId should be undefined');
    assert(gc.epoch === undefined, 'epoch should be undefined');
    gc.free();
  });

  await test('Create group', async () => {
    const gc = await new GroupChat('t_alice', SECRET);
    const gid = await gc.createGroup();
    assert(gid.length > 0, 'group id empty');
    assert(gc.groupId !== undefined, 'groupId undefined');
    assert(gc.epoch !== undefined, 'epoch undefined');
    gc.free();
  });

  await test('Key package generation', async () => {
    const gc = await new GroupChat('t_alice', SECRET);
    const kp = await gc.generateKeyPackage();
    assert(kp.length > 0, 'key package empty');
    gc.free();
  });

  await test('Add member + join', async () => {
    const a = await new GroupChat('t_alice', SECRET);
    const b = await new GroupChat('t_bob', SECRET);
    await a.createGroup();
    const kp = await b.generateKeyPackage();
    const r = await a.addMember(kp);
    assert(r.welcomeMessage.length > 0, 'no welcome');
    assert(r.commitMessage.length > 0, 'no commit');
    const gid = await b.joinGroup(r.welcomeMessage);
    assert(gid.length > 0, 'no group id after join');
    assert(b.groupId !== undefined, 'bob missing groupId');
    a.free(); b.free();
  });

  await test('Encrypt / Decrypt', async () => {
    const a = await new GroupChat('t_alice', SECRET);
    const b = await new GroupChat('t_bob', SECRET);
    await a.createGroup();
    const r = await a.addMember(await b.generateKeyPackage());
    await b.joinGroup(r.welcomeMessage);
    const pt = new TextEncoder().encode('Hello from Alice!');
    const ct = await a.encrypt(pt);
    assert(ct.length > 0, 'ciphertext empty');
    const dec = await b.decrypt(ct);
    assertBytes(dec, pt, 'decrypt mismatch');
    a.free(); b.free();
  });

  await test('Bidirectional messaging', async () => {
    const a = await new GroupChat('t_alice', SECRET);
    const b = await new GroupChat('t_bob', SECRET);
    await a.createGroup();
    const r = await a.addMember(await b.generateKeyPackage());
    await b.joinGroup(r.welcomeMessage);
    const m1 = new TextEncoder().encode('msg-a2b');
    assertBytes(await b.decrypt(await a.encrypt(m1)), m1, 'a→b');
    const m2 = new TextEncoder().encode('msg-b2a');
    assertBytes(await a.decrypt(await b.encrypt(m2)), m2, 'b→a');
    a.free(); b.free();
  });

  await test('Epoch advances', async () => {
    const a = await new GroupChat('t_alice', SECRET);
    const b = await new GroupChat('t_bob', SECRET);
    await a.createGroup();
    const e0 = a.epoch;
    const r = await a.addMember(await b.generateKeyPackage());
    await b.joinGroup(r.welcomeMessage);
    assert(a.epoch > e0, 'epoch not advanced');
    assertEq(a.epoch, b.epoch, 'epoch mismatch');
    a.free(); b.free();
  });

  await test('Three-party group', async () => {
    const a = await new GroupChat('t_alice', SECRET);
    const b = await new GroupChat('t_bob', SECRET);
    const c = await new GroupChat('t_charlie', SECRET);
    await a.createGroup();
    const r1 = await a.addMember(await b.generateKeyPackage());
    await b.joinGroup(r1.welcomeMessage);
    const r2 = await a.addMember(await c.generateKeyPackage());
    await b.processCommit(r2.commitMessage);
    await c.joinGroup(r2.welcomeMessage);
    const msg = new TextEncoder().encode('broadcast');
    const enc = await a.encrypt(msg);
    assertBytes(await b.decrypt(enc), msg, 'bob decrypt');
    assertBytes(await c.decrypt(enc), msg, 'charlie decrypt');
    a.free(); b.free(); c.free();
  });

  await test('Credential rotation', async () => {
    const a = await new GroupChat('t_alice', SECRET);
    const b = await new GroupChat('t_bob', SECRET);
    await a.createGroup();
    const r = await a.addMember(await b.generateKeyPackage());
    await b.joinGroup(r.welcomeMessage);
    const prop = await a.rotateCredentials();
    assert(prop.length > 0, 'proposal empty');
    const commit = await b.processProposal(prop, true);
    assert(commit.length > 0, 'commit empty');
    await a.processCommit(commit);
    const msg = new TextEncoder().encode('post-rotation');
    assertBytes(await b.decrypt(await a.encrypt(msg)), msg, 'post-rotation msg');
    a.free(); b.free();
  });

  const total = passed + failed;
  if (failed === 0) {
    summary.className = 'pass';
    summary.textContent = `ALL ${total} TESTS PASSED`;
    logEvent(`Automated tests: ALL ${total} PASSED`, 'success');
  } else {
    summary.className = 'fail';
    summary.textContent = `${failed} of ${total} TESTS FAILED`;
    logEvent(`Automated tests: ${failed} of ${total} FAILED`, 'error');
  }
}

// ─── Init ───

async function boot() {
  logEvent('Loading WASM module…');
  await init();
  logEvent('WASM module ready', 'success');

  $('#btn-add-participant').addEventListener('click', addParticipant);
  $('#participant-name').addEventListener('keydown', e => { if (e.key === 'Enter') addParticipant(); });
  $('#btn-create-group').addEventListener('click', createGroup);
  $('#btn-add-member').addEventListener('click', addMember);
  $('#btn-send').addEventListener('click', sendMessage);
  $('#msg-text').addEventListener('keydown', e => { if (e.key === 'Enter') sendMessage(); });
  $('#btn-rotate').addEventListener('click', rotateCredentials);
  $('#btn-run-tests').addEventListener('click', runTests);
  $('#btn-clear-log').addEventListener('click', () => { $('#event-log').innerHTML = ''; });
}

boot().then(() => {
  if (new URLSearchParams(location.search).has('autotest')) {
    runTests();
  }
}).catch(e => {
  logEvent(`FATAL: ${e}`, 'error');
  console.error(e);
});
