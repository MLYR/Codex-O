/* ==========================================================================
   Codex-O 原型共享行为 · app.js
   纯 vanilla JS，无外部依赖。职责：
   1) 图标注入  2) 原型工具条  3) 视图状态切换  4) AI 降级/不可用模拟
   5) 弹窗  6) Toast  7) 折叠组  8) Tabs / 分段控件  9) ⌘K 搜索聚焦
   页面配置: window.PROTO = { name, states:[...], aiStates:true, actions:[{id,label}] }
   页面钩子: window.PROTO_ACTIONS = { id: fn } / window.PROTO_HOOKS = { beforeOpen }
   ========================================================================== */

(function () {
  'use strict';

  /* ---------- 1. 图标注入 ---------- */
  function injectIcons(root) {
    (root || document).querySelectorAll('[data-icon]').forEach(function (el) {
      var name = el.getAttribute('data-icon');
      var size = el.getAttribute('data-size') || 16;
      var body = window.ICONS && window.ICONS[name];
      if (!body) return;
      var svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
      svg.setAttribute('class', 'icon');
      svg.setAttribute('width', size);
      svg.setAttribute('height', size);
      svg.setAttribute('viewBox', '0 0 24 24');
      svg.setAttribute('fill', 'none');
      svg.setAttribute('stroke', 'currentColor');
      svg.setAttribute('stroke-width', '1.5');
      svg.setAttribute('stroke-linecap', 'round');
      svg.setAttribute('stroke-linejoin', 'round');
      svg.setAttribute('aria-hidden', 'true');
      svg.innerHTML = body;
      el.replaceWith(svg);
    });
  }
  window.ProtoIcons = injectIcons;

  /* ---------- 6. Toast ---------- */
  var TOAST_ICON = { success: 'check-circle', info: 'info', warning: 'alert' };
  window.showToast = function (opts) {
    var root = document.querySelector('.toast-root');
    if (!root) { root = document.createElement('div'); root.className = 'toast-root'; document.body.appendChild(root); }
    var t = document.createElement('div');
    t.className = 'toast';
    var type = opts.type || 'success';
    t.innerHTML =
      '<span class="t-icon ' + type + '"><svg class="icon" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">' + window.ICONS[TOAST_ICON[type]] + '</svg></span>' +
      '<div><div class="t-title"></div>' + (opts.desc ? '<div class="t-desc"></div>' : '') + '</div>';
    t.querySelector('.t-title').textContent = opts.title || '';
    if (opts.desc) t.querySelector('.t-desc').textContent = opts.desc;
    root.appendChild(t);
    setTimeout(function () {
      t.classList.add('leaving');
      setTimeout(function () { t.remove(); }, 160);
    }, opts.duration || 4000);
  };

  /* ---------- 5. 弹窗 ---------- */
  function openModal(id, opener) {
    var ov = document.getElementById(id);
    if (!ov) return;
    if (window.PROTO_HOOKS && typeof window.PROTO_HOOKS.beforeOpen === 'function') {
      window.PROTO_HOOKS.beforeOpen(id, opener);
    }
    ov.classList.add('open');
    var f = ov.querySelector('[data-autofocus]');
    if (f) setTimeout(function () { f.focus(); }, 60);
  }
  function closeModal(ov) {
    if (ov) ov.classList.remove('open');
  }
  window.ProtoOpenModal = openModal;
  window.ProtoCloseModal = function (id) { closeModal(document.getElementById(id)); };

  document.addEventListener('click', function (e) {
    var opener = e.target.closest('[data-open-modal]');
    if (opener) { openModal(opener.getAttribute('data-open-modal'), opener); return; }
    var closer = e.target.closest('[data-close-modal]');
    if (closer) { closeModal(closer.closest('.modal-overlay')); return; }
    if (e.target.classList && e.target.classList.contains('modal-overlay')) closeModal(e.target);
  });
  document.addEventListener('keydown', function (e) {
    if (e.key === 'Escape') {
      var ov = document.querySelector('.modal-overlay.open');
      if (ov) closeModal(ov);
    }
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
      var s = document.querySelector('[data-hotkey-search]');
      if (s) { e.preventDefault(); s.focus(); }
    }
  });

  /* ---------- 7. 折叠组 ---------- */
  document.addEventListener('click', function (e) {
    var head = e.target.closest('.collapse-head');
    if (head && head.closest('.collapse')) head.closest('.collapse').classList.toggle('open');
  });

  /* ---------- 8. Tabs / 分段控件 ---------- */
  document.addEventListener('click', function (e) {
    var tab = e.target.closest('.tab');
    if (tab && tab.closest('[data-tabs]')) {
      var scope = tab.closest('[data-tabscope]') || document;
      var id = tab.getAttribute('data-tab');
      tab.closest('[data-tabs]').querySelectorAll('.tab').forEach(function (t) { t.classList.toggle('active', t === tab); });
      scope.querySelectorAll('[data-tab-panel]').forEach(function (p) {
        p.hidden = p.getAttribute('data-tab-panel') !== id;
      });
      document.dispatchEvent(new CustomEvent('tab:change', { detail: { id: id } }));
      return;
    }
    var seg = e.target.closest('.segmented button');
    if (seg) {
      seg.closest('.segmented').querySelectorAll('button').forEach(function (b) { b.classList.toggle('active', b === seg); });
      document.dispatchEvent(new CustomEvent('segment:change', { detail: { value: seg.getAttribute('data-seg') || seg.textContent.trim() } }));
    }
  });

  /* ---------- 4. AI 状态模拟（正常 / 降级 / 不可用）---------- */
  var AI_STATE = 'ok';
  function setAiState(state) {
    AI_STATE = state;
    document.body.classList.toggle('ai-degraded', state === 'degraded');
    document.body.classList.toggle('ai-down', state === 'down');
    document.body.classList.remove('banner-dismissed');
    /* 卡片描述等：中文解析 ⇄ 英文原文 */
    document.querySelectorAll('[data-cn]').forEach(function (el) {
      el.textContent = state === 'ok' ? el.getAttribute('data-cn') : (el.getAttribute('data-en') || el.getAttribute('data-cn'));
    });
    /* 横幅配色：不可用 = danger */
    var banner = document.querySelector('.banner');
    if (banner) banner.classList.toggle('danger', state === 'down');
    /* 工具条按钮态 */
    document.querySelectorAll('.proto-btn[data-ai]').forEach(function (b) {
      b.classList.toggle('active', b.getAttribute('data-ai') === state);
    });
    document.dispatchEvent(new CustomEvent('ai:change', { detail: { state: state } }));
  }
  window.ProtoSetAiState = setAiState;

  document.addEventListener('click', function (e) {
    if (e.target.closest('[data-banner-close]')) {
      document.body.classList.add('banner-dismissed');
      return;
    }
    var retry = e.target.closest('[data-ai-retry]');
    if (retry) {
      retry.textContent = '连接中…';
      retry.disabled = true;
      setTimeout(function () {
        setAiState('ok');
        window.showToast({ type: 'success', title: 'AI 服务已恢复', desc: 'gpt-4o-mini 连接正常，可重新解析' });
      }, 900);
    }
  });

  /* ---------- 2/3. 原型工具条 + 视图切换 ---------- */
  var VIEW_LABELS = { normal: '正常', loading: '骨架屏', empty: '空数据', error: '错误' };

  function setView(name) {
    document.querySelectorAll('.content [data-view]').forEach(function (sec) {
      sec.hidden = sec.getAttribute('data-view') !== name;
    });
    document.querySelectorAll('.proto-btn[data-view-btn]').forEach(function (b) {
      b.classList.toggle('active', b.getAttribute('data-view-btn') === name);
    });
    window.scrollTo(0, 0);
    var c = document.querySelector('.content');
    if (c) c.scrollTop = 0;
  }
  window.ProtoSetView = setView;

  function buildProtoBar(proto) {
    var bar = document.createElement('div');
    bar.className = 'proto-bar';
    var html = '<span class="proto-tag">PROTO</span>';
    html += '<span class="w600">' + (proto.name || '') + '</span>';
    html += '<span class="proto-sep"></span><span class="proto-label">视图</span>';
    (proto.states || ['normal']).forEach(function (s) {
      html += '<button class="proto-btn' + (s === 'normal' ? ' active' : '') + '" data-view-btn="' + s + '">' + (VIEW_LABELS[s] || s) + '</button>';
    });
    if (proto.aiStates) {
      html += '<span class="proto-sep"></span><span class="proto-label">AI</span>';
      html += '<button class="proto-btn active" data-ai="ok">正常</button>';
      html += '<button class="proto-btn" data-ai="degraded">降级</button>';
      html += '<button class="proto-btn" data-ai="down">不可用</button>';
    }
    if (proto.actions && proto.actions.length) {
      html += '<span class="proto-sep"></span><span class="proto-label">演示</span>';
      proto.actions.forEach(function (a) {
        html += '<button class="proto-btn" data-proto-action="' + a.id + '">' + a.label + '</button>';
      });
    }
    html += '<span class="ml-auto"></span><a class="proto-link" href="../index.html">← 评审入口</a>';
    bar.innerHTML = html;
    document.body.prepend(bar);

    bar.addEventListener('click', function (e) {
      var vb = e.target.closest('[data-view-btn]');
      if (vb) { setView(vb.getAttribute('data-view-btn')); return; }
      var ab = e.target.closest('[data-ai]');
      if (ab) { setAiState(ab.getAttribute('data-ai')); return; }
      var pb = e.target.closest('[data-proto-action]');
      if (pb && window.PROTO_ACTIONS && typeof window.PROTO_ACTIONS[pb.getAttribute('data-proto-action')] === 'function') {
        window.PROTO_ACTIONS[pb.getAttribute('data-proto-action')]();
      }
    });
  }

  /* ---------- 启动 ---------- */
  document.addEventListener('DOMContentLoaded', function () {
    injectIcons(document);
    var proto = window.PROTO || {};
    if (!proto.bare) buildProtoBar(proto);
  });
})();
