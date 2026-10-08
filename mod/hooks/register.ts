// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
import type { EngineInterface, Register, RenderElement } from 'claude-code'

// recruit's bridge in each member of a team. It decides nothing: it tells recruit what the session knows, serves
// the commands recruit names, asks the model for the model and effort recruit gives, joins to a prompt the note
// recruit gives, and does what recruit asks of the session (a completion, a compaction, the context's breakdown),
// handing the answers over at the next tick. recruit puts it in the members it launches, with RECRUIT_EXE,
// RECRUIT_STATE and RECRUIT_MEMBER; anywhere else it does nothing.

type Effort = 'low' | 'medium' | 'high' | 'xhigh' | 'max'
type Override = { model?: string; effort?: Effort }
// A completion recruit asks for, through the session's own client.
type Ask = { id: string; model: string; system: string; prompt: string; maxTokens: number }
type Reply = {
  text?: string
  register?: { name: string; description: string; argumentHint?: string }[]
  // A member that keeps its prompt alone, without the hint line under it nor the mode labels.
  quiet?: boolean
  override?: Override
  summarize?: Ask
  compact?: string
  // How to count the context's breakdown: estimated by the session, or with the token-count API.
  breakdown?: 'summary' | 'full'
  // What the model reads beside a prompt, the user never sees.
  context?: string
}
// The answers to what recruit asked, for the next tick.
type Summary = { id: string; text?: string; error?: string }
type Compaction = { id: string; done?: boolean; skip?: string; error?: string }
type Handover = { summary?: Summary; compaction?: Compaction }
type Bridge = { exe: string; state: string; member: string }

const nothing: RenderElement = { type: 'Box', props: { display: 'none' } }
// The hint left out. Claude Code still draws its separator after the mode label (`⏵⏵ bypass permissions on · `),
// right before the hint: three blanks drawn over it, from three columns to the left.
const noHint: RenderElement = {
  type: 'Box',
  props: { marginLeft: -3 },
  children: [{ type: 'Text', props: {}, children: ['   '] }],
}

// recruit _mod <event> <state> <member> [command], what the session knows on its standard input.
async function call(
  $: EngineInterface,
  bridge: Bridge,
  event: string,
  input: unknown,
  command?: string,
  timeoutMs = 10_000,
): Promise<Reply> {
  const argv = [bridge.exe, '_mod', event, bridge.state, bridge.member, ...(command ? [command] : [])]
  const r = await $.process.run(argv, { stdin: JSON.stringify(input), timeoutMs })
  if (r.exitCode !== 0) throw new Error(r.stderr.trim() || `recruit _mod ${event}: ${r.exitCode}`)
  return r.stdout.trim() ? (JSON.parse(r.stdout) as Reply) : {}
}

async function bridgeOf($: EngineInterface): Promise<Bridge | undefined> {
  const exe = await $.env.get('RECRUIT_EXE')
  const state = await $.env.get('RECRUIT_STATE')
  const member = await $.env.get('RECRUIT_MEMBER')
  return exe && state && member ? { exe, state, member } : undefined
}

// The agents the session started, as it lists them; null when it could not this time (recruit keeps those it had).
async function agents($: EngineInterface): Promise<unknown[] | null> {
  try {
    return await $.agent.list()
  } catch {
    return null
  }
}

async function tick(
  $: EngineInterface,
  bridge: Bridge,
  own: Override,
  breakdown: Reply['breakdown'],
  handover: Handover,
  compacted: boolean,
): Promise<Reply> {
  const usage = await $.session.usage(breakdown ? { breakdown } : undefined)
  const { tokens, window, percent } = usage.context
  const b = usage.context.breakdown
  const threshold = b && {
    tokens: b.autoCompactThreshold,
    enabled: b.isAutoCompactEnabled,
    window: b.rawMaxTokens,
    model: b.model,
    used: b.totalTokens,
  }
  const input = {
    session: await $.session.id(),
    model: await $.session.model(),
    own,
    context: { tokens, window, percent },
    rateLimits: usage.rateLimits,
    threshold,
    compacted,
    agents: await agents($),
    ...handover,
  }
  return call($, bridge, 'tick', input)
}

async function summarize($: EngineInterface, ask: Ask): Promise<Summary> {
  const { id, ...request } = ask
  try {
    const r = await $.model.complete({ ...request, timeoutMs: 15_000 })
    return r.isAnswered ? { id, text: r.text } : { id, error: r.reason }
  } catch (error) {
    // A request the engine refuses to send: a model the organization blocks, a bad cap.
    return { id, error: String(error) }
  }
}

async function compact($: EngineInterface, id: string): Promise<Compaction> {
  try {
    const r = await $.session.compact()
    return 'skip' in r && r.skip !== undefined ? { id, skip: r.skip } : { id, done: true }
  } catch (error) {
    // A turn running refuses it.
    return { id, error: String(error) }
  }
}

// What recruit joins to a prompt, if anything; nothing when it does not answer in time.
async function submitted($: EngineInterface, bridge: Bridge, text: string, compacted: boolean): Promise<Reply | undefined> {
  try {
    return await call($, bridge, 'submit', { session: await $.session.id(), text, compacted }, undefined, 3_000)
  } catch {
    return undefined
  }
}

export const register: Register = on => {
  let bridge: Bridge | undefined
  let override: Override = {}
  // The model and effort of the session's last request, as the engine made it, before recruit's.
  let own: Override = {}
  const commands: string[] = []
  let quiet = false
  let handover: Handover = {}
  // The conversation's compactions, and how many recruit heard of, with a tick or a prompt.
  let compactions = 0
  let told = 0

  on('session.start', async ($, e, next) => {
    bridge = await bridgeOf($)
    if (!bridge) return next(e)
    const known = (await $.command.list()).map(c => c.name)
    const reply = await call($, bridge, 'start', { commands: known, session: await $.session.id() })
    for (const spec of reply.register ?? []) {
      await $.command.register({ ...spec, immediate: true })
      commands.push(spec.name)
    }
    quiet = reply.quiet === true
    if (quiet) $.ui.invalidate('ui.render')
    const found = bridge
    let running = false
    // Ticks failed in a row. The first is not shown: while recruit is being replaced (brew upgrade), its executable
    // is missing for a moment, and the next tick finds the new one.
    let failed = 0
    let breakdown: Reply['breakdown']
    $.clock.every(2000, async () => {
      if (running) return
      running = true
      const sent = handover
      handover = {}
      try {
        const count = compactions
        const reply = await tick($, found, own, breakdown, sent, count > told)
        told = Math.max(told, count)
        override = reply.override ?? {}
        breakdown = reply.breakdown
        // The main contact changed while the team runs.
        if (reply.quiet !== undefined && reply.quiet !== quiet) {
          quiet = reply.quiet
          $.ui.invalidate('ui.render')
        }
        if (reply.summarize) void summarize($, reply.summarize).then(summary => (handover = { ...handover, summary }))
        if (reply.compact) void compact($, reply.compact).then(compaction => (handover = { ...handover, compaction }))
        // The error shown, if any, is over.
        if (failed > 1) $.ui.status(undefined)
        failed = 0
      } catch (error) {
        // Handed over at the next tick instead, unless newer answers came meanwhile.
        handover = { ...sent, ...handover }
        failed += 1
        // Claude Code puts the plugin's name before it.
        if (failed > 1) $.ui.status(String(error))
      } finally {
        running = false
      }
    })
    return next(e)
  })

  on('command.run', async ($, e, next) => {
    if (!bridge || !commands.includes(e.command)) return next(e)
    try {
      return { text: (await call($, bridge, 'command', { args: e.args }, e.command)).text ?? '' }
    } catch (error) {
      return { text: `recruit: ${error}`, exitCode: 1 }
    }
  })

  // Whatever brings a prompt: the user, a teammate's message, a task's notice.
  on('prompt.submit', async ($, e, next) => {
    if (!bridge) return next(e)
    const count = compactions
    const reply = await submitted($, bridge, e.text, count > told)
    if (reply) told = Math.max(told, count)
    const context = reply?.context
    return next(context ? { ...e, context: [...(e.context ?? []), context] } : e)
  })

  // The main conversation's, once installed: a precompute installs nothing.
  on('session.compact', async ($, e, next) => {
    const r = await next(e)
    if (bridge && !e.agentId && e.trigger !== 'precompute' && r.skip === undefined) compactions += 1
    return r
  })

  on('ui.render', { component: 'PromptHint' }, ($, e, next) => (quiet ? noHint : next(e)))
  on('ui.render', { component: 'SessionMode' }, ($, e, next) => (quiet ? nothing : next(e)))

  // The member's own requests only: a subagent keeps the model it was given.
  on('turn.step', async function* ($, e, next) {
    if (e.agentId) return yield* next(e)
    own = { model: e.model, effort: typeof e.effort === 'string' ? e.effort : undefined }
    return yield* next({ ...e, ...override })
  })
}
