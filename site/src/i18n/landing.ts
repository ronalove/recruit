// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
// Texts of the home page, English and French (informal "tu"). The docs have their own pages under src/content/docs.
export type Lang = 'en' | 'fr';

type State = 'working' | 'needs' | 'done';

export interface Strings {
  lang: Lang;
  htmlLang: string;
  locale: string;
  title: string;
  description: string;
  docs: string;
  github: string;
  otherLang: { label: string; code: Lang };
  start: string;
  heroTitle: string;
  heroAccent: string;
  heroText: string;
  cta: string;
  copy: string;
  copied: string;
  copyLabel: string;
  chart: {
    label: string;
    you: string;
    coordinator: { name: string; state: State; task: string };
    specialists: { name: string; state: State; task: string }[];
    states: Record<State, string>;
  };
  compare: {
    title: string;
    one: string;
    team: string;
    rows: [string, string][];
  };
  tour: {
    title: string;
    tabs: { id: 'dashboard' | 'journal' | 'menu' | 'team'; label: string; text: string; alt: string }[];
  };
  perks: { icon: 'resume' | 'grow' | 'star'; title: string; text: string }[];
  proof: { title: string; text: string; members: string[] };
  final: { title: string; small: string };
  footer: { license: string; docs: string; releases: string; github: string };
}

const en: Strings = {
  lang: 'en',
  htmlLang: 'en',
  locale: 'en_US',
  title: 'recruit · Big apps need a team. Recruit one.',
  description:
    'recruit turns Claude Code into a lasting team of specialists. You set the direction; they split the work, build it, test it and ship it, day after day.',
  docs: 'Docs',
  github: 'GitHub',
  otherLang: { label: 'Français', code: 'fr' },
  start: 'Get started',
  heroTitle: 'Big apps need a team.',
  heroAccent: 'Recruit one.',
  heroText:
    'recruit turns Claude Code into a lasting team of specialists. You set the direction; they split the work, build it, test it and ship it, day after day.',
  cta: 'Build your first team',
  copy: 'Copy',
  copied: 'Copied',
  copyLabel: 'Copy the install command',
  chart: {
    label: 'An example team: you talk to a coordinator, which works with four specialists.',
    you: 'You',
    coordinator: { name: 'Coordinator', state: 'working', task: 'Plans Apple Pay with the team' },
    specialists: [
      { name: 'Backend', state: 'working', task: 'Building the payment endpoint' },
      { name: 'Frontend', state: 'needs', task: 'Wants to add the Stripe SDK' },
      { name: 'Tester', state: 'done', task: '✓ All checkout tests pass' },
      { name: 'Release', state: 'done', task: '✓ Shipped v2.4.1' },
    ],
    states: { working: 'working', needs: 'needs you', done: 'done' },
  },
  compare: {
    title: 'One Claude writes code. A team ships products.',
    one: 'One agent',
    team: 'A recruit team',
    rows: [
      ['Does everything, one thing at a time.', 'Specialists work side by side, each on its own part.'],
      ['You write every prompt and chase every result.', 'You brief a coordinator. It runs the rest and reports back.'],
      ['Its attention spreads over the whole codebase.', 'Each specialist knows its area deeply, and stays on it.'],
      ['You keep track of progress in your head.', "You see who's on what, live."],
    ],
  },
  tour: {
    title: 'See it work',
    tabs: [
      {
        id: 'dashboard',
        label: "Who's on what",
        text: "Who's working, who's waiting for you, who just finished, and what each one is doing right now.",
        alt: 'The recruit dashboard: one card per team member, with its state, model, effort and what it is doing.',
      },
      {
        id: 'journal',
        label: 'What they tell each other',
        text: 'The team talks to itself so you don’t have to relay. Every message, as it happens.',
        alt: 'The recruit journal: the messages the team members send each other, as they happen.',
      },
      {
        id: 'menu',
        label: 'Reshape the team',
        text: 'A new specialist, a different role, a stronger model: change the team in seconds, while it works.',
        alt: 'The recruit menu: a member’s card with its model, effort, role and instructions, next to the list of the team.',
      },
      {
        id: 'team',
        label: 'Your coordinator',
        text: 'You talk to one member. It plans, delegates, follows up, and tells you when it’s done.',
        alt: 'A recruit team in tmux: the coordinator on the left, the dashboard and the journal on the right.',
      },
    ],
  },
  perks: [
    {
      icon: 'resume',
      title: 'Same team tomorrow',
      text: 'Every specialist picks up where it left off. Close your terminal; the work goes on.',
    },
    {
      icon: 'grow',
      title: 'Grows with your app',
      text: 'Add a reviewer before launch or a writer for the docs, without stopping anyone.',
    },
    {
      icon: 'star',
      title: 'The right people from day one',
      text: 'Pick your kind of project, or describe it and Claude proposes the roles.',
    },
  ],
  proof: {
    title: 'Made by its own team.',
    text: 'A coordinator, two developers, a reviewer and an ops agent build every release of recruit. They built this page too.',
    members: ['coordinator', 'dev-cli', 'dev-mod', 'review', 'ops'],
  },
  final: { title: 'Build your first team.', small: 'Free and open source · For Claude Code · macOS and Linux' },
  footer: { license: 'AGPL-3.0 license', docs: 'Docs', releases: 'Releases', github: 'GitHub' },
};

const fr: Strings = {
  lang: 'fr',
  htmlLang: 'fr',
  locale: 'fr_FR',
  title: 'recruit · Les grandes applis demandent une équipe. Recrute la tienne.',
  description:
    'recruit fait de Claude Code une équipe de spécialistes qui dure. Tu donnes le cap ; ils se répartissent le travail, le construisent, le testent et le livrent, jour après jour.',
  docs: 'Docs',
  github: 'GitHub',
  otherLang: { label: 'English', code: 'en' },
  start: 'Commencer',
  heroTitle: 'Les grandes applis demandent une équipe.',
  heroAccent: 'Recrute la tienne.',
  heroText:
    'recruit fait de Claude Code une équipe de spécialistes qui dure. Tu donnes le cap ; ils se répartissent le travail, le construisent, le testent et le livrent, jour après jour.',
  cta: 'Monte ta première équipe',
  copy: 'Copier',
  copied: 'Copié',
  copyLabel: 'Copier la commande d’installation',
  chart: {
    label: 'Un exemple d’équipe : tu parles à un coordinateur, qui travaille avec quatre spécialistes.',
    you: 'Toi',
    coordinator: { name: 'Coordinateur', state: 'working', task: 'Prépare Apple Pay avec l’équipe' },
    specialists: [
      { name: 'Backend', state: 'working', task: 'Construit l’API de paiement' },
      { name: 'Frontend', state: 'needs', task: 'Veut ajouter le SDK Stripe' },
      { name: 'Testeur', state: 'done', task: '✓ Tous les tests du paiement passent' },
      { name: 'Release', state: 'done', task: '✓ v2.4.1 livrée' },
    ],
    states: { working: 'au travail', needs: 't’attend', done: 'fini' },
  },
  compare: {
    title: 'Un Claude écrit du code. Une équipe livre un produit.',
    one: 'Un agent',
    team: 'Une équipe recruit',
    rows: [
      ['Fait tout, une chose à la fois.', 'Des spécialistes travaillent côte à côte, chacun sur sa partie.'],
      [
        'Tu écris chaque demande et tu cours après chaque résultat.',
        'Tu parles à un coordinateur. Il mène le reste et te rend compte.',
      ],
      [
        'Son attention se disperse sur tout le code.',
        'Chaque spécialiste connaît son domaine à fond, et y reste.',
      ],
      ['Tu suis l’avancement de tête.', 'Tu vois qui fait quoi, en direct.'],
    ],
  },
  tour: {
    title: 'Regarde-les travailler',
    tabs: [
      {
        id: 'dashboard',
        label: 'Qui fait quoi',
        text: 'Qui travaille, qui t’attend, qui vient de finir, et ce que fait chacun en ce moment.',
        alt: 'Le tableau de bord de recruit : une carte par membre, avec son état, son modèle, son effort et ce qu’il fait.',
      },
      {
        id: 'journal',
        label: 'Ce qu’ils se disent',
        text: 'L’équipe se parle directement : tu n’as rien à relayer. Chaque message, au moment où il part.',
        alt: 'Le journal de recruit : les messages que les membres de l’équipe s’envoient, au moment où ils partent.',
      },
      {
        id: 'menu',
        label: 'Remanier l’équipe',
        text: 'Un nouveau spécialiste, un autre rôle, un modèle plus fort : l’équipe change en quelques secondes, sans s’arrêter.',
        alt: 'Le menu de recruit : la fiche d’un membre avec son modèle, son effort, son rôle et ses instructions, à côté de la liste de l’équipe.',
      },
      {
        id: 'team',
        label: 'Ton coordinateur',
        text: 'Tu parles à un seul membre. Il planifie, délègue, relance, et te dit quand c’est fait.',
        alt: 'Une équipe recruit dans tmux : le coordinateur à gauche, le tableau de bord et le journal à droite.',
      },
    ],
  },
  perks: [
    {
      icon: 'resume',
      title: 'La même équipe demain',
      text: 'Chaque spécialiste reprend là où il s’était arrêté. Ferme ton terminal : le travail continue.',
    },
    {
      icon: 'grow',
      title: 'Elle grandit avec ton appli',
      text: 'Ajoute un relecteur avant le lancement ou un rédacteur pour la doc, sans arrêter personne.',
    },
    {
      icon: 'star',
      title: 'Les bonnes personnes dès le premier jour',
      text: 'Choisis ton type de projet, ou décris-le et Claude propose les rôles.',
    },
  ],
  proof: {
    title: 'Fait par sa propre équipe.',
    text: 'Un coordinateur, deux développeurs, un relecteur et un agent ops construisent chaque version de recruit. Cette page aussi.',
    members: ['coordinateur', 'dev-cli', 'dev-mod', 'review', 'ops'],
  },
  final: { title: 'Monte ta première équipe.', small: 'Libre et open source · Pour Claude Code · macOS et Linux' },
  footer: { license: 'Licence AGPL-3.0', docs: 'Docs', releases: 'Versions', github: 'GitHub' },
};

export const strings: Record<Lang, Strings> = { en, fr };
