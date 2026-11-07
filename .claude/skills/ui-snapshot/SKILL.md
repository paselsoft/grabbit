---
name: ui-snapshot
description: Avvia Grabbit su uno schermo virtuale (Xvfb) con yt-dlp, Deno e ffmpeg finti, clicca e scrive come un utente, e salva screenshot della finestra per vedere davvero l'interfaccia. Usala dopo ogni modifica all'interfaccia egui (src/app.rs, schede Scarica e Ascolta, barra del player), quando un criterio di una storia è marcato [snap], o quando l'utente chiede di vedere, fotografare o provare una schermata.
license: MIT
metadata:
  inspired-by: https://github.com/daveebbelaar/bama (build-and-verify)
---

# Screenshot dell'interfaccia di Grabbit

Compilare non basta: un layout storto, un testo tagliato o una barra che non si
ridisegna si vedono solo guardando la finestra. Questo script la avvia in un
ambiente isolato e la fotografa dopo ogni passo.

## 1. Prepara l'ambiente (una volta per sessione)

```bash
apt-get install -y xvfb xdotool imagemagick libxkbcommon-x11-0 libgl1 libegl1 libgl1-mesa-dri libasound2-dev
```

Poi `cargo build` (lo script usa `target/debug/grabbit`).

Check: `which Xvfb xdotool import` stampa tre percorsi e il build finisce.

## 2. Fotografa

```bash
python3 .claude/skills/ui-snapshot/scripts/snapshot.py --out <cartella-scratchpad>/shots \
  [--music <cartella con brani>] [--lang it|en] PASSI...
```

Passi: `shot:nome`, `click:X,Y`, `wait:secondi`, `key:Return`, `type:testo`.

- Lo schermo è 1000x1000; la finestra (780x780) è in alto a sinistra. Fai
  sempre prima un `shot:` e leggi le coordinate dall'immagine, poi clicca.
- Coordinate utili con la finestra di serie: scheda «Scarica» (55,94),
  scheda «Ascolta» (155,94). Il primo album della libreria è intorno a (150,230),
  le righe successive ogni 67 px.
- Configurazione, dati e strumenti stanno in una cartella temporanea: la
  configurazione vera non viene toccata. Ricerca e download non funzionano
  (yt-dlp è finto e YouTube non è raggiungibile): si provano layout, schede,
  libreria e player.
- L'audio va su un'uscita ALSA muta che suona molto più veloce del tempo reale:
  un brano di 30 s finisce in un paio di secondi. Per fotografare la barra del
  player usa un brano lungo (un'ora, creato con ffmpeg) o scatta subito dopo il clic.
- Brani di prova con copertina, numero di traccia e artista:

```bash
ffmpeg -loglevel error -f lavfi -i "sine=frequency=440:duration=30" -i copertina.jpg \
  -map 0 -map 1 -c:a aac -c:v mjpeg -disposition:v attached_pic \
  -metadata title="Brano 1" -metadata artist="Artista" -metadata track=1 "Album/1 - Brano 1.m4a"
```

Check: lo script esce con 0 e stampa il percorso di ogni immagine.

## 3. Guarda gli screenshot

Apri ogni immagine con lo strumento Read e controlla: testi interi e non
tagliati, allineamenti, nessun simbolo mostrato come quadratino (i font di serie
di egui non hanno ▶ ⏸ ♫: le icone si disegnano), stati vuoti e di errore.
Se Grabbit si chiude, leggi `app.log` nella cartella di uscita.

Gotcha: egui ridisegna solo quando succede qualcosa. Se dopo un clic la
schermata non cambia finché non muovi il mouse, manca un `request_repaint`
(è successo con la barra del player).

## Done when

Hai uno screenshot di ogni schermata toccata dalla modifica, li hai guardati, e
hai corretto o segnalato ogni difetto visto.
