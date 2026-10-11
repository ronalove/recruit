// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
import type { EngineInterface, PromptBox, Register, RenderElement } from 'claude-code'

// recruit's bridge in each member of a team. It decides nothing: it tells recruit what the session knows, serves
// the commands recruit names, asks the model for the model and effort recruit gives, joins to a prompt the note
// recruit gives, and does what recruit asks of the session (a completion, a compaction, « continue » after a failed
// turn, the context's breakdown), handing the answers over at the next tick. recruit puts it in the members it launches, with RECRUIT_EXE,
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
  // What the answer to the compaction tells back: the server's own (auto), the context before (percent); and, for an
  // automatic one, what the summary keeps.
  compactAsk?: { auto?: boolean; percent?: number; instructions?: string }
  // « continue » to send, unless the prompt box holds a draft or a turn runs.
  resume?: string
  // How to count the context's breakdown: estimated by the session, or with the token-count API.
  breakdown?: 'summary' | 'full'
  // What the model reads beside a prompt, the user never sees.
  context?: string
}
// The answers to what recruit asked, for the next tick.
type Summary = { id: string; text?: string; error?: string }
type Compaction = { id: string; done?: boolean; skip?: string; error?: string; auto?: boolean; percent?: number }
// 'busy' and 'unsupported' are codes recruit says in the user's language.
type Resumption = { id: string; sent?: boolean; draft?: boolean; error?: string }
// A /compact the person typed, that recruit did not ask for: how big the conversation was and became, if known.
type Typed = { tokensBefore?: number; tokensAfter?: number }
type Handover = { summary?: Summary; compaction?: Compaction; resumption?: Resumption; typed?: Typed }
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

async function compact(
  $: EngineInterface,
  id: string,
  ask: Reply['compactAsk'],
  running: () => boolean,
): Promise<Compaction> {
  // Told back in the answer, for the dashboard's trace.
  const who = { ...(ask?.auto ? { auto: true } : {}), ...(ask?.percent !== undefined ? { percent: ask.percent } : {}) }
  // A turn running refuses it: not a failure, the member went back to work; recruit asks again if it still must.
  if (running()) return { id, skip: 'busy', ...who }
  try {
    const r = await $.session.compact(ask?.instructions ? { instructions: ask.instructions } : undefined)
    return 'skip' in r && r.skip !== undefined ? { id, skip: r.skip, ...who } : { id, done: true, ...who }
  } catch (error) {
    // A turn that began since the check refused it all the same.
    if (running()) return { id, skip: 'busy', ...who }
    return { id, error: String(error), ...who }
  }
}

// « continue », as the person would type it to take up a turn that failed, into an empty prompt box only: the box
// read, then the prompt submitted with no wait in between. A plugin's prompt goes through the engine's queue, never
// through the box, and would wait for a running turn to end: refused while one runs, before and after the read.
async function resume($: EngineInterface, id: string, running: () => boolean): Promise<Resumption> {
  if (running()) return { id, error: 'busy' }
  let box: PromptBox
  try {
    box = await $.prompt.read()
  } catch (error) {
    // A Claude Code without the call, said rather than nothing; read never rejects otherwise.
    return { id, error: error instanceof TypeError ? 'unsupported' : String(error) }
  }
  if (box.text.trim()) return { id, draft: true }
  // A turn the person started while the box was read: « continue » would wait for its end.
  if (running()) return { id, error: 'busy' }
  try {
    const r = await $.prompt.submit({ text: 'continue', asUser: true })
    return r.drop !== undefined ? { id, error: r.drop } : { id, sent: true }
  } catch (error) {
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
  // The main conversation's turn running, if any.
  let turn: string | undefined

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
        const turnRuns = () => turn !== undefined
        if (reply.compact) {
          void compact($, reply.compact, reply.compactAsk, turnRuns).then(
            compaction => (handover = { ...handover, compaction }),
          )
        }
        if (reply.resume) {
          void resume($, reply.resume, turnRuns).then(resumption => (handover = { ...handover, resumption }))
        }
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

  // The main conversation's, once installed: a precompute installs nothing. The person's own /compact is handed over
  // for the trace; recruit's goes through the plugin, the engine's at its threshold is shown by the panel.
  on('session.compact', async ($, e, next) => {
    const r = await next(e)
    if (bridge && !e.agentId && e.trigger !== 'precompute' && r.skip === undefined) {
      compactions += 1
      if (e.trigger === 'manual') {
        handover = { ...handover, typed: { tokensBefore: r.tokensBefore, tokensAfter: r.tokensAfter } }
      }
    }
    return r
  })

  // The main loop's alone: a subagent's run raises no `turn.start`. Every end comes, an API error's too.
  on('turn.start', async ($, e, next) => {
    turn = e.turnId
    return next(e)
  })
  on('turn.complete', async ($, e, next) => {
    if (!e.agentId && e.turnId === turn) turn = undefined
    return next(e)
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
