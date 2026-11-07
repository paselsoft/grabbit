---
name: user-stories
description: Backlog di storie utente per Grabbit (docs/STORIES.md), una azione per storia, con criteri di accettazione verificabili ([unit], [ci], [snap], [manual]) e milestone. Usala prima di una funzione nuova o grande (per esempio la ricerca su Spotify), quando l'utente chiede cosa fare dopo, una lista di funzioni, un piano o delle priorità, o quando una richiesta contiene più cose insieme da spezzare.
license: MIT
metadata:
  adapted-from: https://github.com/daveebbelaar/bama (skills/plan/user-stories, MIT, Dave Ebbelaar)
---

# Storie utente per Grabbit

Le schermate mostrano com'è l'app; le storie dicono cosa ci fa una persona.
Si costruiscono e si provano storie, una per commit (o per pull request).

Leggi `README.it.md`, `docs/STORIES.md` e `docs/GLOSSARY.md` se esistono, e il
codice che la funzione tocca (`src/app.rs` per l'interfaccia, `src/search.rs`,
`src/downloader.rs`, `src/player.rs`, `src/tools.rs`). Scrivi `docs/STORIES.md`.

## Formato

```markdown
### S-014 Cercare un album su Spotify
Come utente, scelgo «Spotify · album» in «Cerca su», scrivo un artista e vedo gli album con copertina e anno.
Dipende: S-013
Accettazione:
- [unit] La risposta di esempio di Spotify diventa 12 risultati con titolo, artista, anno e link.
- [snap] Ogni riga mostra copertina, «Album · 2019» e i pulsanti Scarica e +.
- [ci] cargo clippy e cargo test passano su Linux, Windows e macOS.
- [manual] Sull'iMac (High Sierra) «Scarica» salva l'album in M4A con le copertine.
Stato: da fare
```

- Una azione per storia: «Filtrare la discografia per singoli», non «Gestire
  la discografia».
- Ogni riga di accettazione dice come si verifica, e «funziona bene» non è
  verificabile:
  - `[unit]` un test in `cargo test` (anche con un finto yt-dlp, come in
    `downloader.rs`), senza rete;
  - `[ci]` la CI di GitHub su Linux, Windows e macOS;
  - `[snap]` uno screenshot con la skill `ui-snapshot`;
  - `[manual]` la deve provare l'utente: rete vera (YouTube, YouTube Music,
    Spotify), audio vero, aggiornamento automatico, un Mac o Windows reale.
- Tutto ciò che dipende da YouTube o Spotify veri è `[manual]`: da questo
  ambiente non si raggiungono. Scrivi anche cosa deve guardare l'utente
  (il «Registro dettagliato», la riga in fondo con le versioni).
- Ricorda le piattaforme: macOS vecchi (High Sierra, yt-dlp per Python,
  QuickJS), Windows, Linux AppImage. Una storia che tocca strumenti esterni ha
  una riga per i Mac vecchi.
- `Dipende:` (facoltativo) elenca le storie da fare prima.
- Stato: `da fare`, `in corso`, `fatta`, `scartata`.

Check: ogni storia ha una sola azione e ogni riga di accettazione inizia con
un tag.

## Priorità e milestone

Raggruppa le storie in milestone. La milestone 1 è la più piccola versione che
l'utente può davvero usare: ognuna finisce con una release (`0.x.0`) che
Grabbit installa da solo con l'aggiornamento automatico.

```markdown
## Milestone 1: ricerca Spotify di base (0.3.0)
S-013, S-014, S-015
```

Ordina per valore per l'utente e per rischio: prima ciò che toglie un dubbio
(per esempio se un'API risponde senza chiave), poi il resto.

Check: ogni storia sta in una milestone e la milestone 1 si può rilasciare da sola.

## Dopo

Per ogni storia che aggiunge una schermata, un'azione o del testo, chiama la
skill `ux-design` prima di scrivere codice. Lavora una storia per commit:
codice, `cargo fmt`, `cargo clippy --all-targets`, `cargo test`, e per Windows
`cargo clippy --all-targets --target x86_64-pc-windows-gnu`; poi `ui-snapshot`
per le righe `[snap]`; segna la storia `fatta` solo quando ogni riga non
`[manual]` è verificata, e chiedi all'utente le righe `[manual]`.

## Done when

`docs/STORIES.md` ha le storie con criteri verificabili, raggruppate in
milestone, e hai detto all'utente qual è la prima storia da fare.
