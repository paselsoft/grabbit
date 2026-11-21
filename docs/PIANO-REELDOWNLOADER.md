# Piano: idee da ReelDownloader per Grabbit

*Analisi di `ReelDownloader.app` (versione 1.0, costruita con Gemini) per scaricare video da
Facebook e RaiPlay. Grabbit di riferimento: 0.7.0.*

## In breve

ReelDownloader è un programma piccolo e mirato. Contiene un solo file Python (`app.py`, 218 righe)
con un'interfaccia Tkinter, che chiama lo yt-dlp installato sul Mac. Le sue idee migliori sono
quattro:

1. **Sottotitoli e trascrizione:** scarica i sottotitoli (anche quelli automatici) in `.srt` e ne
   ricava un `.txt` pulito, da leggere o copiare.
2. **Cookie salvati per un sito:** esporta una volta i cookie di Facebook da Chrome in un file e
   usa quello. Così non chiede ogni volta l'accesso al Portachiavi e scarica i video che
   richiedono l'accesso.
3. **Nome del file a scelta**, per un singolo link.
4. **Una cartella dedicata** ai video di Facebook.

Grabbit sa già scaricare da Facebook e RaiPlay, perché passa qualsiasi link a yt-dlp. Queste idee
però lo renderebbero **molto più comodo** proprio per quei siti. Tutte sono realizzabili, senza
rischi per High Sierra.

---

## 1. Com'è fatto ReelDownloader

| Parte | Cosa fa |
| --- | --- |
| `Contents/MacOS/launcher` | script bash che avvia `python3.11` del Python di python.org (`/Library/Frameworks/Python.framework/Versions/3.11`) |
| `Contents/Resources/app.py` | finestra Tkinter: URL, titolo facoltativo, due caselle, quattro pulsanti, registro |
| `Info.plist` | `local.paolo.reeldownloader`, versione 1.0, nessun requisito di macOS |

**Il flusso:** incolli l'URL e premi **Scarica**. Lui lancia:

```text
yt-dlp [--cookies ~/.fb_cookies.txt | --cookies-from-browser chrome]
       -f "bv*[ext=mp4]+ba[ext=m4a]/bv*+ba/b" --merge-output-format mp4
       [--write-subs --write-auto-subs --sub-langs "it.*,en.*" --convert-subs srt]
       -o "/Users/paolo/Movies/Facebook_Reels/<titolo o reel_ID>.%(ext)s" URL
```

Poi trasforma ogni `.srt` della cartella in un `.txt` senza numeri e senza orari.

**Le funzioni:**

| Funzione | Dettaglio |
| --- | --- |
| Incolla | mette nel campo il testo degli appunti |
| Titolo file (opzionale) | nome del file; se vuoto `reel_<id>` |
| Usa cookie Facebook | se c'è `~/.fb_cookies.txt` usa quello, altrimenti i cookie di Chrome |
| Scarica trascrizione | sottotitoli italiani e inglesi, anche automatici, in `.srt` più un `.txt` pulito |
| Aggiorna Cookie | esporta i cookie da Chrome e tiene solo quelli di `facebook.com` in `~/.fb_cookies.txt` (permessi 600) |
| Apri Cartella, Pulisci | apre la cartella di destinazione; svuota campi e registro |

### Limiti di ReelDownloader (da non copiare)

- **Percorsi scritti nel codice:** `/Users/paolo/...` e il Python 3.11 in una cartella precisa.
  Su un altro Mac, o dopo un aggiornamento di Python, non parte.
- **Dipende da Python e yt-dlp installati a mano**, che non si aggiornano da soli. Grabbit invece
  scarica e aggiorna yt-dlp da solo.
- **Cookie:** «Aggiorna Cookie» scrive per un attimo **tutti** i cookie di Chrome di tutti i siti
  in `/tmp/all_cookies_reelapp.txt`, che altri utenti del Mac possono leggere, prima di filtrarli.
  In Grabbit il file va scritto solo nella cartella dei dati, leggibile solo da te.
- La conversione `.srt` → `.txt` rilegge tutta la cartella, non solo il video appena scaricato.
- Un link alla volta, nessun avanzamento a parte il registro, nessuna gestione dei messaggi
  d'errore di yt-dlp.
- **RaiPlay** non ha nulla di specifico: funziona perché yt-dlp riconosce i link RaiPlay.

## 2. Cosa ha già Grabbit

| Esigenza | In Grabbit oggi |
| --- | --- |
| Scaricare un link Facebook o RaiPlay | sì: ogni link va a yt-dlp, in modo Video come MP4 H.264/AAC |
| Incolla, apri la cartella, registro | sì («Incolla», «Apri cartella dei download», «Registro dettagliato») |
| Cookie del browser | sì, ma per **tutti** i download: «Opzioni avanzate › Usa i cookie del browser» |
| Avanzamento, più link insieme, notifica a fine download | sì |
| yt-dlp sempre aggiornato, nessuna installazione | sì |
| Sottotitoli e trascrizione | **no** |
| Nome del file a scelta | **no** |
| Cartella diversa per sito | **no** |
| Cookie salvati per un solo sito | **no** |

## 3. Idee da portare

### 3.1 Sottotitoli e trascrizione (consigliata per prima)

- **Cosa:** in modo **Video**, una casella **«Sottotitoli e trascrizione»** con la scelta delle
  lingue (italiano, inglese o entrambe). Grabbit:
  - scarica i sottotitoli del video, sia quelli caricati sia quelli automatici:
    `--write-subs --write-auto-subs --sub-langs "it.*,en.*" --convert-subs srt`;
  - li mette anche dentro l'MP4 con `--embed-subs`, così QuickTime li mostra;
  - accanto al video salva un **`.txt` pulito**, senza numeri, orari e righe ripetute (i
    sottotitoli automatici di YouTube ripetono molto). Serve per leggere, cercare o riassumere.
- **Dove serve:** RaiPlay (molti programmi hanno i sottotitoli), YouTube (lezioni, interviste),
  Facebook (quando ci sono).
- **Come:**
  - gli argomenti di yt-dlp in `build_args`;
  - la conversione `.srt` → `.txt` in Rust, **solo sui file del video appena scaricato**: yt-dlp
    stampa i percorsi dei sottotitoli e li legge Grabbit;
  - nell'elenco download, accanto a «Mostra nel Finder», un pulsante **«Apri la trascrizione»**.
- **Prove:**
  - `[unit]` uno `.srt` con numeri, orari e righe ripetute diventa un testo pulito;
  - `[unit]` gli argomenti di yt-dlp ci sono con la casella attiva e mancano con la casella spenta;
  - `[manual]` un video RaiPlay con sottotitoli e un video YouTube con sottotitoli automatici.
- **Lavoro:** piccolo-medio.

### 3.2 Cookie salvati per sito (Facebook, RaiPlay)

- **Il problema di oggi:** in Grabbit i cookie del browser valgono per tutti i download, e sul
  Mac ogni lettura dei cookie di Chrome può chiedere l'accesso al Portachiavi. Per Facebook (reel
  privati o di gruppi) e RaiPlay (contenuti che richiedono l'accesso) servono, per YouTube di
  solito no.
- **Cosa:** in «Opzioni avanzate», una sezione **«Accesso ai siti»**:
  - un elenco di siti (Facebook, Instagram, RaiPlay, più altri a scelta), ognuno con
    **«Aggiorna l'accesso da Chrome / Safari / Firefox»**;
  - Grabbit esporta una volta i cookie **solo di quel sito** in un file nella sua cartella dei
    dati, leggibile solo da te, e mostra la data dell'ultimo aggiornamento;
  - quando scarichi un link di quel sito, Grabbit usa `--cookies <file di quel sito>`. Gli altri
    siti non ricevono quei cookie e il Portachiavi non viene interrogato a ogni download;
  - se un download fallisce perché serve l'accesso, il messaggio lo dice e propone di aggiornare.
- **Come:** come in ReelDownloader (`yt-dlp --cookies-from-browser X --cookies tmp --skip-download
  https://www.facebook.com/`), ma con il file temporaneo nella cartella dei dati di Grabbit, non in
  `/tmp`, poi filtrato sul dominio e cancellato.
- **Prove:**
  - `[unit]` il filtro tiene solo le righe del dominio e l'intestazione, e il file ha i permessi 600;
  - `[unit]` un link Facebook riceve `--cookies` con il file di Facebook, un link YouTube no;
  - `[manual]` un reel che senza accesso fallisce.
- **Lavoro:** medio.

### 3.3 Riconoscere il sito e dare messaggi chiari

- **Cosa:** Grabbit riconosce il sito di ogni link e lo scrive nella riga del download, per
  esempio «Facebook · reel» o «RaiPlay». In più:
  - per Facebook e RaiPlay il modo **Video** è il naturale: con «Audio» scelto, un avviso lo
    ricorda;
  - i link **RaiPlay Sound** (radio e podcast) vanno invece in **audio**, e potrebbero entrare in
    «Ascolta» in un gruppo **Podcast**;
  - i fallimenti tipici di questi siti diventano messaggi in italiano con il rimedio:

| Messaggio di yt-dlp (in breve) | Messaggio di Grabbit |
| --- | --- |
| `DRM` / `This video is DRM protected` | «Questo contenuto RaiPlay è protetto (DRM) e non si può scaricare.» |
| `login required` / `cookies` / `registered users` | «Serve l'accesso a <sito>: aggiorna l'accesso in Opzioni avanzate.» |
| `not available in your country` | «Non disponibile nel tuo paese.» |

- **Lavoro:** medio. Il riconoscimento del sito è piccolo, la tabella dei messaggi cresce con
  l'uso.

### 3.4 Nome del file a scelta

- **Cosa:** quando nei link c'è **un solo link**, compare un campo **«Nome del file
  (facoltativo)»**. Per i reel di Facebook il titolo spesso è il testo del post, lungo, o
  «Video di …», e un nome scelto aiuta. Più avanti: **«Rinomina»** su un download finito.
- **Come:** il nome, pulito con `sanitize`, diventa il modello `-o "<nome>.%(ext)s"`; vale anche
  per sottotitoli e trascrizione, che prendono lo stesso nome.
- **Lavoro:** piccolo.

### 3.5 Cartelle per sito

- **Cosa:** come «Musica in cartelle Artista / Album», un'opzione **«Video in cartelle per sito»**:
  - `Filmati/Facebook/…`, `Filmati/RaiPlay/<programma>/…`, `Filmati/YouTube/<canale>/…`;
  - di serie i video vanno nella cartella **Filmati** del Mac, come fa ReelDownloader con
    `Movies/Facebook_Reels`, e non in quella della musica.
- **Come:** il modello di yt-dlp con `%(extractor_key)s` e `%(series,uploader)s`, con nomi di
  cartella puliti.
- **Lavoro:** piccolo.

## 4. Cosa non portare

- L'interfaccia Tkinter e lo script di avvio legato a Python 3.11: Grabbit è un'app unica che non
  richiede installazioni.
- Il file con tutti i cookie in `/tmp`.
- La conversione di tutti gli `.srt` della cartella a ogni download.

## 5. Piano a versioni

Questo piano va d'accordo con quello di Dropall (`docs/PIANO-DROPALL.md`). Le idee qui sono più
piccole e più vicine a come usi Grabbit, quindi propongo di farle **prima** del download manager:

| Versione | Contenuto | Lavoro | Rischio su High Sierra |
| --- | --- | --- | --- |
| **0.8.0** | Sottotitoli e trascrizione (§3.1), nome del file (§3.4), cartelle per sito con i video in Filmati (§3.5) | medio | nessuno |
| **0.9.0** | Cookie salvati per sito (§3.2), riconoscimento del sito e messaggi chiari (§3.3), RaiPlay Sound in audio | medio | nessuno |
| poi | Il piano Dropall: limite di velocità e download manager… | | |

**Come si prova:** da qui Facebook e RaiPlay non si raggiungono. Le prove con link veri
`[manual]` le fai tu sull'iMac con alcuni link di esempio: un reel pubblico, un reel che richiede
l'accesso, un programma RaiPlay con sottotitoli, un contenuto RaiPlay protetto. Io preparo test
automatici con risposte di yt-dlp registrate e screenshot dell'interfaccia.

## 6. Decisioni che servono da te

1. **Sottotitoli:** ti bastano italiano e inglese, o vuoi scegliere la lingua ogni volta?
2. **Cartelle:** i video in `Filmati` (con le cartelle per sito) e la musica in `Musica/Grabbit`?
   Oppure tutto nella stessa cartella come oggi?
3. **Accesso ai siti:** oltre a Facebook e RaiPlay, ti servono Instagram o altri?
4. **RaiPlay Sound:** ti interessa scaricare radio e podcast e ascoltarli in «Ascolta»?

## 7. Nota

Facebook e RaiPlay hanno termini d'uso che limitano lo scaricamento, e alcuni contenuti RaiPlay
sono protetti da DRM: Grabbit, come yt-dlp, non aggira mai il DRM. Vale la stessa regola che
Grabbit mostra in fondo alla finestra: solo contenuti che hai il diritto di scaricare.
