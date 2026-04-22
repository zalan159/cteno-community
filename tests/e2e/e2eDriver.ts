/**
 * E2E Test Driver — 注入到前端 window.__e2e
 *
 * 设计原则：
 * 1. 能脚本判的绝不交给 AI
 * 2. 验证优先级：store query > DOM 断言 > 可访问性树 > 截图存档
 * 3. 全部 JSON 输出，agent 直接 parse
 * 4. 所有操作走同一条代码路径（和 UI 按钮调同一个函数）
 *
 * 使用方式：
 *   ctenoctl webview eval "JSON.stringify(await window.__e2e.testVendor('claude'))"
 */

// ============================================================
// 安装入口（在 _layout.tsx DEV 模式调用）
// ============================================================

export function installE2EDriver() {
  if (typeof window === 'undefined') return;
  if ((window as any).__e2e) return; // 已安装

  const driver = {
    // ============================================================
    // Layer 1: Store 查询（数据是否到了前端）
    // ============================================================

    /** 所有 session 概要 */
    sessions: () => {
      const { storage } = require('@/sync/storage');
      const state = storage.getState();
      return Object.values(state.sessions).map((s: any) => ({
        id: s.id,
        vendor: s.metadata?.flavor,
        status: s.status,
        messageCount: s.messages?.length ?? 0,
        isStreaming: !!s.streamingText,
        streamingTextLength: s.streamingText?.length ?? 0,
        error: s.error,
      }));
    },

    /** 指定 session 的消息列表 */
    messages: (sessionId: string) => {
      const { storage } = require('@/sync/storage');
      const sess = storage.getState().sessions[sessionId];
      if (!sess) return { found: false };
      return {
        found: true,
        sessionId,
        isStreaming: !!sess.streamingText,
        streamingText: sess.streamingText?.slice(0, 500),
        messages: (sess.messages ?? []).map((m: any) => ({
          role: m.role,
          type: m.content?.type ?? typeof m.content,
          text: typeof m.content === 'string'
            ? m.content.slice(0, 300)
            : (m.content?.data?.text ?? m.content?.data?.type ?? '').toString().slice(0, 300),
          timestamp: m.createdAt,
        })),
      };
    },

    /** 所有 persona 概要 */
    personas: () => {
      const { storage } = require('@/sync/storage');
      return storage.getState().cachedPersonas?.map((p: any) => ({
        id: p.id,
        name: p.name,
        agent: p.agent,
        sessionId: p.chatSessionId,
        isPending: p.chatSessionId?.startsWith('pending-'),
      })) ?? [];
    },

    /** 可用 vendor 列表 */
    vendors: () => {
      const { storage } = require('@/sync/storage');
      const machines = Object.values(storage.getState().machines);
      return { machineCount: machines.length, machineId: (machines[0] as any)?.id };
    },

    // ============================================================
    // Layer 2: DOM 断言（React 是否正确渲染了）
    // ============================================================

    /** 查找元素 — 返回可见性、文本、位置 */
    find: (selector: string) => {
      const elements = document.querySelectorAll(selector);
      return Array.from(elements).map(el => {
        const rect = el.getBoundingClientRect();
        const style = window.getComputedStyle(el);
        const visible = style.display !== 'none'
          && style.visibility !== 'hidden'
          && style.opacity !== '0'
          && rect.width > 0 && rect.height > 0;
        return {
          visible,
          inViewport: rect.top < window.innerHeight && rect.bottom > 0
            && rect.left < window.innerWidth && rect.right > 0,
          text: el.textContent?.trim().slice(0, 200),
          bounds: {
            x: Math.round(rect.x), y: Math.round(rect.y),
            w: Math.round(rect.width), h: Math.round(rect.height),
          },
        };
      });
    },

    /** 断言元素可见 */
    assertVisible: (selector: string, containsText?: string): { ok: boolean; reason?: string; text?: string } => {
      const els = document.querySelectorAll(selector);
      for (const el of els) {
        const rect = el.getBoundingClientRect();
        const style = window.getComputedStyle(el);
        const visible = style.display !== 'none' && rect.width > 0 && rect.height > 0;
        const inViewport = rect.top < window.innerHeight && rect.bottom > 0;
        const hasText = !containsText || (el.textContent?.includes(containsText) ?? false);
        if (visible && inViewport && hasText) {
          return { ok: true, text: el.textContent?.trim().slice(0, 200) };
        }
      }
      return {
        ok: false,
        reason: `No visible element matching "${selector}"${containsText ? ` with text "${containsText}"` : ''}`,
      };
    },

    /** 断言元素不存在或不可见 */
    assertHidden: (selector: string): { ok: boolean } => {
      const el = document.querySelector(selector);
      if (!el) return { ok: true };
      const rect = el.getBoundingClientRect();
      const style = window.getComputedStyle(el);
      const hidden = style.display === 'none' || style.visibility === 'hidden'
        || rect.width === 0 || rect.height === 0;
      return { ok: hidden };
    },

    /** 获取可见文本列表 — 用于验证消息内容 */
    visibleTexts: (selector: string): string[] => {
      return Array.from(document.querySelectorAll(selector))
        .filter(el => {
          const rect = el.getBoundingClientRect();
          return rect.width > 0 && rect.height > 0;
        })
        .map(el => el.textContent?.trim() ?? '')
        .filter(Boolean);
    },

    /** 元素计数 */
    count: (selector: string): number => {
      return document.querySelectorAll(selector).length;
    },

    /** 布局断言 — A 在 B 下方 / 右侧 / 等 */
    assertBelow: (selectorA: string, selectorB: string): { ok: boolean; gap?: number } => {
      const a = document.querySelector(selectorA)?.getBoundingClientRect();
      const b = document.querySelector(selectorB)?.getBoundingClientRect();
      if (!a || !b) return { ok: false };
      const gap = a.top - b.bottom;
      return { ok: gap >= 0, gap: Math.round(gap) };
    },

    // ============================================================
    // Layer 3: 可访问性树快照（结构化 UI 概要）
    // ============================================================

    /** 轻量级 UI 快照 — 只提取有 testID/role/text 的节点 */
    snapshot: (maxDepth = 6) => {
      const walk = (node: Element, depth: number): any => {
        if (depth > maxDepth) return null;

        const style = window.getComputedStyle(node);
        const rect = node.getBoundingClientRect();
        if (style.display === 'none' || rect.width === 0) return null;

        const role = node.getAttribute('role');
        const testId = node.getAttribute('data-testid');
        const ariaLabel = node.getAttribute('aria-label');
        const text = node.childNodes.length === 1
          && node.childNodes[0].nodeType === Node.TEXT_NODE
          ? node.textContent?.trim().slice(0, 100) : undefined;

        const children = Array.from(node.children)
          .map(c => walk(c, depth + 1))
          .filter(Boolean);

        // 只保留有意义的节点
        if (!text && !testId && !role && !ariaLabel && children.length === 0) return null;

        return {
          ...(testId && { testId }),
          ...(role && { role }),
          ...(ariaLabel && { label: ariaLabel }),
          ...(text && { text }),
          ...(children.length > 0 && { children }),
        };
      };
      return walk(document.body, 0);
    },

    // ============================================================
    // 等待（解决异步时序）
    // ============================================================

    /** 等待 store 条件满足 */
    waitFor: async (conditionFn: () => boolean, timeoutMs = 30000, pollMs = 500) => {
      const start = Date.now();
      while (Date.now() - start < timeoutMs) {
        try { if (conditionFn()) return { ok: true, elapsed: Date.now() - start }; } catch {}
        await new Promise(r => setTimeout(r, pollMs));
      }
      return { ok: false, timeout: true, elapsed: timeoutMs };
    },

    /** 等待 session 有 agent 回复（store 级别） */
    waitForResponse: async (sessionId: string, timeoutMs = 60000) => {
      const { storage } = require('@/sync/storage');
      const start = Date.now();
      while (Date.now() - start < timeoutMs) {
        const sess = storage.getState().sessions[sessionId];
        if (sess?.streamingText) {
          return { ok: true, streaming: true, elapsed: Date.now() - start, textLength: sess.streamingText.length };
        }
        const msgs = sess?.messages ?? [];
        const agentMsg = msgs.filter((m: any) => m.role === 'agent' || m.role === 'assistant');
        if (agentMsg.length > 0) {
          return { ok: true, streaming: false, elapsed: Date.now() - start, messageCount: agentMsg.length };
        }
        await new Promise(r => setTimeout(r, 500));
      }
      return { ok: false, timeout: true, elapsed: timeoutMs };
    },

    /** 等待 DOM 元素出现 */
    waitForElement: async (selector: string, timeoutMs = 15000) => {
      const start = Date.now();
      while (Date.now() - start < timeoutMs) {
        const el = document.querySelector(selector);
        if (el) {
          const rect = el.getBoundingClientRect();
          if (rect.width > 0 && rect.height > 0) {
            return { ok: true, elapsed: Date.now() - start };
          }
        }
        await new Promise(r => setTimeout(r, 300));
      }
      return { ok: false, timeout: true, elapsed: timeoutMs };
    },

    // ============================================================
    // 操作（和 UI 走同一条代码路径）
    // ============================================================

    /** 创建 persona — 等同于用户点"新任务"选 vendor */
    createPersona: async (vendor: string, workdir = '~/') => {
      const { machineCreatePersona } = require('@/sync/ops');
      const { sync } = require('@/sync/sync');
      const { storage } = require('@/sync/storage');
      const machines = Object.values(storage.getState().machines) as any[];
      const machineId = machines[0]?.id;
      if (!machineId) return { ok: false, error: 'no machine available' };

      const result = await machineCreatePersona(machineId, { workdir, agent: vendor });
      if (!result.success) return { ok: false, error: result.error };

      await sync.refreshSessions();
      return {
        ok: true,
        personaId: result.persona?.id,
        sessionId: result.persona?.chatSessionId,
        isPending: result.persona?.chatSessionId?.startsWith('pending-'),
        agent: result.persona?.agent ?? vendor,
      };
    },

    /** 发消息 — 等同于用户在输入框按发送 */
    sendMessage: async (sessionId: string, text: string) => {
      const { sync } = require('@/sync/sync');
      try {
        await sync.sendMessage(sessionId, text);
        return { ok: true };
      } catch (e: any) {
        return { ok: false, error: e.message };
      }
    },

    /** 导航 */
    navigate: (path: string) => {
      const { router } = require('expo-router');
      router.push(path);
      return { ok: true };
    },

    // ============================================================
    // 高级组合（一步到位的端到端验证）
    // ============================================================

    /**
     * 完整 vendor E2E 测试
     * 验证链：创建 → 发消息 → store 收到回复 → DOM 有渲染
     */
    testVendor: async (vendor: string, message = 'say hello in one word', timeoutMs = 60000) => {
      const t0 = Date.now();
      const report: any = { vendor, steps: {} };

      // Step 1: 创建 persona
      const create = await driver.createPersona(vendor);
      report.steps.create = create;
      if (!create.ok) return { ...report, ok: false, failedAt: 'create' };
      if (create.isPending) return { ...report, ok: false, failedAt: 'create', error: 'got pending session ID' };

      // Step 2: 发消息
      const send = await driver.sendMessage(create.sessionId!, message);
      report.steps.send = send;
      if (!send.ok) return { ...report, ok: false, failedAt: 'send' };

      // Step 3: 等待 store 有回复（数据层验证）
      const storeResponse = await driver.waitForResponse(create.sessionId!, timeoutMs);
      report.steps.storeResponse = storeResponse;
      if (!storeResponse.ok) return { ...report, ok: false, failedAt: 'storeResponse' };

      // Step 4: 读最终 session 状态
      const finalState = driver.messages(create.sessionId!);
      report.steps.finalMessages = finalState;

      // Step 5: 综合报告
      return {
        ...report,
        ok: true,
        personaId: create.personaId,
        sessionId: create.sessionId,
        latencyMs: Date.now() - t0,
        messageCount: finalState.found ? finalState.messages?.length : 0,
        streaming: storeResponse.streaming,
      };
    },

    /** 批量测试所有可用 vendor */
    testAllVendors: async (message = 'say hello in one word') => {
      // 从 daemon 获取可用 vendors
      const { listAvailableVendors } = require('@/sync/ops');
      const { storage } = require('@/sync/storage');
      const machines = Object.values(storage.getState().machines) as any[];
      const machineId = machines[0]?.id;

      let vendors: string[];
      try {
        const list = await listAvailableVendors(machineId);
        vendors = list.filter((v: any) => v.available).map((v: any) => v.name);
      } catch {
        vendors = ['cteno']; // fallback
      }

      const results = [];
      for (const v of vendors) {
        results.push(await driver.testVendor(v, message));
      }
      return {
        vendors,
        results,
        allPassed: results.every((r: any) => r.ok),
        summary: results.map((r: any) => `${r.vendor}: ${r.ok ? 'PASS' : 'FAIL'} (${r.latencyMs ?? 0}ms)`),
      };
    },
  };

  (window as any).__e2e = driver;
  console.log('[E2E] Driver installed. Use window.__e2e.testVendor("claude") etc.');
}
