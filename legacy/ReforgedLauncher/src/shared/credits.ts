// Everyone the launcher's Credits page thanks, in English and Brazilian Portuguese. This is the only
// place the list lives. Plain data, no links: the page opens project pages by link name, and the
// main process maps those names to fixed URLs (src/main/links.ts).

import type { Language } from "./types";

export type Localized = Readonly<Record<Language, string>>;

export type CreditRole = "maintainer" | "creator" | "contributor";

export interface CreditPerson {
  name: string; // as shown
  github: string; // GitHub handle
  role: CreditRole;
  note: Localized;
}

export interface CreditProject {
  name: string;
  link: "revived_source" | "mystic_source" | "undaunted_source";
  text: Localized;
  people: readonly CreditPerson[];
}

export interface CreditSoftware {
  name: string;
  author: string | Localized;
  license: string | null; // SPDX identifier; null when the project publishes no license file
  note: Localized;
}

export const PROJECTS: readonly CreditProject[] = [
  {
    name: "Dauntless Revived",
    link: "revived_source",
    text: {
      en: "This launcher is a modified version of the Dauntless Revived launcher: its design, its look, the way servers are joined (invites, account keys, the pinned TLS relay, the gateway and the server status) and much of its code.",
      "pt-BR": "Este launcher é uma versão modificada do launcher do Dauntless Revived: o design, o visual, a forma de entrar nos servidores (convites, chaves de conta, o relay com TLS fixado, o gateway e o status do servidor) e boa parte do código.",
    },
    people: [
      { name: "mixutin", github: "mixutin", role: "maintainer", note: { en: "Maintains Dauntless Revived: its launcher, gateway, content server, server kit and docs.", "pt-BR": "Mantém o Dauntless Revived: o launcher, o gateway, o servidor de conteúdo, o kit de servidor e a documentação." } },
      { name: "anxo0", github: "anxo0", role: "contributor", note: { en: "The Dauntless-inspired launcher design, styled settings menus and saved servers in Dauntless Revived.", "pt-BR": "O design do launcher inspirado no Dauntless, os menus de configuração e os servidores salvos no Dauntless Revived." } },
      { name: "Vvoidddd", github: "Vvoidddd", role: "contributor", note: { en: "Using an existing game folder by pasting its path, and the first in-game chat server, in Dauntless Revived.", "pt-BR": "Usar uma pasta do jogo já existente colando o caminho, e o primeiro servidor de chat do jogo, no Dauntless Revived." } },
    ],
  },
  {
    name: "Mystic Paradox",
    link: "mystic_source",
    text: {
      en: "The Dauntless 1.14.7 port: the runtime that runs the game on a private server, the backend that answers the 1.14.7 client, and the Director that starts Ramsgate, the Training Grounds and the hunts. Dauntless Reforged Revival uses them at a pinned commit with a small set of patches.",
      "pt-BR": "O port para o Dauntless 1.14.7: o runtime que roda o jogo num servidor privado, o backend que responde ao cliente 1.14.7 e o Director que inicia Ramsgate, o Training Grounds e as caçadas. O Dauntless Reforged Revival os usa num commit fixo com um pequeno conjunto de patches.",
    },
    people: [
      { name: "Pranav Karande (MysticFox)", github: "pranav158", role: "creator", note: { en: "Ported Undaunted to Dauntless 1.12.0 and 1.14.7: the runtime hooks, the backend, the Director and the game-data tools.", "pt-BR": "Portou o Undaunted para o Dauntless 1.12.0 e 1.14.7: os hooks do runtime, o backend, o Director e as ferramentas de game data." } },
    ],
  },
  {
    name: "Undaunted",
    link: "undaunted_source",
    text: {
      en: "The project both of the above are built on: the first Dauntless private server.",
      "pt-BR": "O projeto em que os dois acima se baseiam: o primeiro servidor privado de Dauntless.",
    },
    people: [
      { name: "gwog (Gregory Morford)", github: "SyST3MDeV", role: "creator", note: { en: "Created Undaunted: the server-mode DLL that turns the game client into a game server, the deploy server, the metagame and the original launcher.", "pt-BR": "Criou o Undaunted: a DLL de modo servidor que transforma o cliente do jogo num servidor, o deploy server, o metagame e o launcher original." } },
      { name: "EisigesEis", github: "EisigesEis", role: "contributor", note: { en: "Metagame work: inventory and loadouts, progression and mastery, invite codes and the admin API.", "pt-BR": "Trabalho no metagame: inventário e loadouts, progressão e maestria, códigos de convite e a API de administração." } },
    ],
  },
];

// Open-source software in the launcher and in the runtime it installs. The full license texts are
// in THIRD-PARTY-NOTICES.txt, which ships with the launcher.
export const SOFTWARE: readonly CreditSoftware[] = [
  { name: "Electron", author: { en: "Electron contributors", "pt-BR": "Colaboradores do Electron" }, license: "MIT", note: { en: "The app framework. The Chromium and Node.js notices ship next to the launcher as LICENSES.chromium.html.", "pt-BR": "O framework do aplicativo. Os avisos do Chromium e do Node.js acompanham o launcher em LICENSES.chromium.html." } },
  { name: "update-electron-app", author: "Electron contributors", license: "MIT", note: { en: "Checks for launcher updates.", "pt-BR": "Procura atualizações do launcher." } },
  { name: "electron-squirrel-startup", author: "Mark Lee", license: "Apache-2.0", note: { en: "Handles the Windows installer's start-up events.", "pt-BR": "Trata os eventos de inicialização do instalador do Windows." } },
  { name: "GitHub Octicons", author: "GitHub, Inc.", license: "MIT", note: { en: "The GitHub mark on the project button.", "pt-BR": "O símbolo do GitHub no botão do projeto." } },
  { name: "MinHook", author: "Tsuda Kageyu", license: "BSD-2-Clause", note: { en: "The function-hooking library in the game runtime.", "pt-BR": "A biblioteca de hooks de funções do runtime do jogo." } },
  { name: "Hacker Disassembler Engine 64", author: "Vyacheslav Patkov", license: "BSD-2-Clause", note: { en: "Part of MinHook.", "pt-BR": "Parte do MinHook." } },
  { name: "coldloader-proxy", author: "denuvosanctuary", license: "Apache-2.0", note: { en: "The base of the winmm.dll loader that starts the runtime.", "pt-BR": "A base do carregador winmm.dll que inicia o runtime." } },
  { name: "Dumper-7", author: { en: "Encryqed and contributors", "pt-BR": "Encryqed e colaboradores" }, license: null, note: { en: "The Unreal Engine SDK generator the runtime is built against. Each host runs it on their own game; it is not shipped.", "pt-BR": "O gerador do SDK da Unreal Engine usado para compilar o runtime. Cada host o roda no próprio jogo; ele não é distribuído." } },
];

export function localized(v: string | Localized, lang: Language): string {
  return typeof v === "string" ? v : v[lang] ?? v.en;
}
