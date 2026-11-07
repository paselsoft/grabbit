---
name: ux-design
description: Passaggio di UX per una funzione di Grabbit prima di scriverla, cioè nomi (glossario), flusso, dove si apre (riga, riquadro, sezione richiudibile, scheda, menu a tendina, finestra), stati, testi in italiano e inglese con tr!, suggerimenti al passaggio del mouse e accessibilità in egui. Usala prima di una storia che aggiunge una schermata, un'azione o del testo, quando l'utente chiede una revisione dell'interfaccia, nomi o testi più chiari, o se una cosa deve aprirsi in una finestra a parte.
license: MIT
metadata:
  adapted-from: https://github.com/daveebbelaar/bama (skills/design/ux-design, MIT, Dave Ebbelaar)
---

# UX per Grabbit

Una funzione che funziona può comunque confondere. Questo passaggio decide come
si chiamano le cose, come ci si muove, dove si apre e cosa dice, prima del codice.
Il risultato è un blocco `UX:` nella storia in `docs/STORIES.md` e i termini nuovi
in `docs/GLOSSARY.md`. Per rivedere una schermata già fatta, fai gli stessi passi
su uno screenshot (skill `ui-snapshot`) ed elenca i problemi, dal più grave.

## 1. Dai i nomi

Ogni cosa ha un solo nome, in italiano e in inglese, e vive nel glossario.

```markdown
| Termine | Inglese | Significa | Non dire |
| --- | --- | --- | --- |
| Discografia | Discography | Tutte le uscite di un artista | catalogo, opere |
| Uscita | Release | Un album, singolo o EP | prodotto, pubblicazione |
| Registro dettagliato | Detailed log | Le righe di yt-dlp | log, console |
```

- Le parole dell'utente, non del codice: «brano», non «track» o «job».
- Una parola per idea in pulsanti, titoli, suggerimenti ed errori.
- Nomi già in uso in Grabbit da rispettare: Scarica, Ascolta, Cerca su, Link,
  Opzioni avanzate, Registro dettagliato, Download contemporanei.

Check: ogni nome che la storia mette sullo schermo è nel glossario, in tutte e due le lingue.

## 2. Il flusso

Scrivilo in una riga, da ogni punto di ingresso a una fine visibile.

```text
Cerca su «YouTube Music · artista» -> scrivi -> Invio o «Cerca» -> Album (12) -> «Scarica» -> riga in Download con avanzamento
```

- Valori di serie sensati; chiedi solo ciò che non si può indovinare.
- Ogni passo ha un modo per tornare indietro: «Annulla», «Pulisci risultati», «Tutti gli album».
- La fine si vede: una riga nuova, un messaggio verde, un brano che parte.

Check: il flusso sta in una riga e ogni passo ha la sua via d'uscita.

## 3. Dove si apre

Scegli la prima voce che va bene: più leggero è meglio.

| La funzione | Usa |
| --- | --- |
| Cambia un valore già sullo schermo | Nella riga (casella, menu a tendina `ComboBox`, pulsante piccolo) |
| Fa parte del lavoro principale di una scheda | Un riquadro (`card`) nella scheda |
| Serve di rado o solo agli esperti | Sezione richiudibile («Opzioni avanzate», «Registro dettagliato») |
| È un'attività diversa con una sua lista | Una scheda nuova accanto a «Scarica» e «Ascolta» |
| Deve restare visibile mentre si lavora altrove | Una barra fissa (come quella del player) |
| Un'azione rara che cancella dati senza ritorno | Una conferma (`egui::Modal`) con il nome dell'azione sul pulsante |

- Un messaggio che non richiede azioni va nella riga o nel riquadro, mai in una finestra.
- Una sola finestra modale alla volta; Invio conferma, Esc annulla.

Check: il blocco `UX:` nomina il posto e la riga della tabella che l'ha scelto.

## 4. Gli stati

Ogni vista toccata ha parole e aspetto per ogni stato:

- **Vuoto**: a cosa serve e la prima azione («Nessun brano riproducibile in questa cartella…»).
- **In corso**: rotellina con il motivo («Leggo la cartella…») e «Annulla» se si può.
- **Errore**: vicino al problema, con il rimedio (vedi il messaggio sulla copia
  in sola lettura di macOS in `selfupdate.rs`), mai solo il testo tecnico.
- **Troppo**: nomi lunghi troncati (`Label::truncate`) con il testo intero al
  passaggio del mouse; liste lunghe dentro una `ScrollArea` con altezza massima.
- **Fatto**: un segno visibile subito («Copiato negli appunti ✓», riga verde).

Check: ogni stato ha uno screenshot o dei dati di prova che lo mostrano.

## 5. Le parole

- Tutto il testo passa da `tr!("italiano", "english")`: le due frasi dicono la stessa cosa.
- I pulsanti iniziano con un verbo: «Scarica», «Aggiungi ai link», «Copia tutto il registro».
- I puntini «…» quando il comando chiede altro prima di agire («Sfoglia…»).
- Errori: cosa è successo e come rimediare, senza colpe e senza gergo.
- Conferme che ripetono l'azione: «Eliminare 3 brani?» con «Elimina» e «Annulla», mai «Sì» e «No».
- Simboli: i font di serie di egui non hanno ▶ ⏸ ⏮ ♫ (escono quadratini).
  Le icone si disegnano con il painter, come `icon_button` in `src/app.rs`.

Check: rileggi ogni testo ad alta voce, in tutte e due le lingue.

## 6. Mouse e accessibilità

- Ogni pulsante solo icona ha `.on_hover_text(...)` che dice cosa fa, con un verbo.
- Ogni controllo senza testo visibile (icone disegnate, caselle senza etichetta,
  righe cliccabili) ha `widget_info(|| WidgetInfo::labeled(Role::…, attivo, testo))`
  con le stesse parole dello schermo: lo legge il lettore di schermo (AccessKit).
- Il significato non sta solo nel colore: lo stato di un download è anche scritto.
- I campi di testo hanno `labelled_by` con la loro etichetta.
- La dimensione del testo segue «Opzioni avanzate → Dimensione del testo»:
  controlla che a «Molto grande» nulla si sovrapponga.

Check: ogni controllo nuovo ha un nome per il lettore di schermo e un suggerimento se è solo icona.

## 7. Il blocco UX

Aggiungilo sotto le righe di accettazione della storia:

```markdown
UX:
- Flusso: Cerca su «Spotify · album» -> scrivi -> Invio -> risultati -> «Scarica»
- Posto: nel riquadro «Cerca» (fa parte del lavoro principale della scheda Scarica)
- Stati: vuoto «Nessun album per «…».»; errore «Spotify non risponde: riprova più tardi.»
- Termini nuovi: Album (Album), Uscita (Release)
```

## Done when

La storia ha il blocco `UX:`, i termini nuovi sono nel glossario in tutte e due
le lingue, e ogni controllo nuovo ha testo, suggerimento e nome accessibile decisi.
