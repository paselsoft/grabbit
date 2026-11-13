# Storie utente di Grabbit

Una azione per storia. Ogni riga di accettazione dice come si verifica:
`[unit]` test in `cargo test`, `[ci]` CI di GitHub, `[snap]` screenshot con la
skill `ui-snapshot`, `[manual]` da provare sul computer vero (rete, YouTube, Mac).

## Milestone 1: download più chiari e ordinati (0.3.0)

S-001, S-002, S-003

### S-001 Vedere «Scarica» e l'avanzamento senza scorrere
Come utente, nella scheda Scarica trovo sempre in basso una barra con «Scarica»,
il numero di link pronti e, durante il download, l'avanzamento totale con «Annulla».
Accettazione:
- [snap] Con la finestra di serie (780×780) la barra è visibile senza scorrere, con «Scarica» e «Nessun link» o «N link».
- [snap] Durante un download la barra mostra «Annulla», una barra di avanzamento e «X di N completati · 45%»; ogni brano in corso mostra la sua percentuale.
- [snap] «Vedi elenco» porta l'elenco dei download in vista; la barra non compare nella scheda Ascolta.
- [ci] cargo clippy e cargo test passano su Linux, Windows e macOS.
UX:
- Flusso: scegli un risultato o incolla un link -> la barra dice «1 link» -> «Scarica» -> la barra mostra l'avanzamento -> «3 di 3 completati»
- Posto: barra fissa in basso (deve restare visibile mentre si lavora sopra), sopra quella del player
- Stati: vuoto «Nessun link: cerca o incolla un link»; in corso «X di N completati · Y non riusciti»; fatto «N completati» in verde
- Termini nuovi: Barra dei download (Download bar)
Stato: fatta

### S-002 Saltare i brani già scaricati
Come utente, se scarico di nuovo un brano o un album che ho già, Grabbit non lo
riscarica e nell'elenco scrive «Già scaricato».
Accettazione:
- [unit] Dopo un download riuscito il brano è nell'archivio; un secondo download dello stesso link con lo stesso formato non avvia yt-dlp e finisce con «Già scaricato».
- [unit] Se il file è stato cancellato, o il formato è diverso (video invece di audio, MP3 invece di M4A), il brano si riscarica.
- [unit] Con «Salta i brani già scaricati» spento si riscarica sempre.
- [manual] Sul Mac: scarica un album, poi riscaricalo; ogni brano compare subito come «Già scaricato».
UX:
- Posto: nella riga del download (cambia lo stato di una riga); l'opzione in «Opzioni avanzate» (di rado)
- Stati: «Già scaricato · 03 - Titolo.m4a» in verde
- Termini nuovi: Già scaricato (Already downloaded)
Stato: fatta

### S-003 Salvare la musica in cartelle Artista / Album
Come utente, quando scarico audio i brani vanno in `Artista/Album/01 - Titolo.m4a`,
come in iTunes e Plex; posso spegnerlo in «Opzioni avanzate».
Accettazione:
- [unit] Un album di YouTube Music (playlist OLAK5uy_) va in «Artista/Album/01 - Titolo», con l'artista senza « - Topic».
- [unit] Un brano singolo usa i metadati di yt-dlp: «artista/album/titolo», con «Singoli» se manca l'album.
- [unit] Una playlist che non è un album resta nella cartella con il nome della playlist; i video e l'opzione spenta restano come prima.
- [snap] «Ascolta» mostra gli album dentro le cartelle degli artisti, con l'artista preso dalla cartella se i file non hanno il tag.
- [manual] Sul Mac: scarica un album e un brano da YouTube Music e controlla le cartelle.
UX:
- Posto: casella in «Opzioni avanzate», attiva di serie
- Termini nuovi: Cartelle Artista / Album (Artist / Album folders), Singoli (Singles)
Stato: fatta

## Milestone 2: libreria ordinata in «Ascolta» (0.4.0)

S-004, S-005

### S-004 Vedere la musica raggruppata per artista
Come utente, in «Ascolta» vedo un gruppo richiudibile per ogni artista, in ordine alfabetico,
con i suoi album dal più recente e i singoli in fondo, segnati «Singolo».
Accettazione:
- [unit] L'artista del gruppo è la cartella dell'artista (`Artista/Album`), altrimenti il primo artista dei tag («Renga Nek, Francesco Renga, Nek» → «Renga Nek»).
- [unit] Ordine: artisti alfabetici, poi album (più di 3 brani) dal più recente, poi singoli; senza artista e brani sciolti in fondo.
- [snap] Ogni album mostra anno, «Singolo» se lo è, l'artista solo se diverso dal gruppo, brani e durata.
- [manual] Sul Mac: la libreria vera appare per artista e i due «Cani Sciolti» si distinguono.
UX:
- Posto: nel riquadro di «Ascolta», gruppi richiudibili (`CollapsingHeader`)
- Termini nuovi: Uscita (Release), Singolo (Single)
Stato: fatta

### S-005 Cercare nella libreria
Come utente, scrivo in «Cerca nella libreria» e vedo solo gli artisti, album o brani che contengono il testo.
Accettazione:
- [snap] «cani» mostra solo i due «Cani Sciolti», nei loro gruppi aperti.
- [snap] Nessun risultato: «Nessun artista, album o brano con «…».»
UX:
- Posto: campo in cima all'elenco degli album
Stato: fatta

## Milestone 3: download più comodi (0.5.0)

S-006, S-007, S-008, S-009

### S-006 Agire su un singolo brano dell'elenco
Come utente, su ogni riga dei download ho l'azione giusta: «Annulla» mentre scarica,
«Mostra nel Finder» quando è finito, «Riprova» se non è riuscito.
Accettazione:
- [unit] «Annulla» su un brano ferma solo quello; «Riprova» lo riscarica con la stessa cartella e lo stesso nome.
- [snap] Righe in corso con «Annulla», finite con «Mostra nel Finder» (su Linux «Apri la cartella»), annullate con «Riprova»; in basso «Riprova i non riusciti (N)».
- [manual] Sul Mac «Mostra nel Finder» apre il Finder con il file selezionato.
Stato: fatta

### S-007 Sapere quando il download è finito
Come utente, se Grabbit è in secondo piano, a fine download ricevo una notifica e l'icona
nel Dock saltella; durante il download il titolo della finestra mostra la percentuale.
Accettazione:
- [manual] Sul Mac, con un'altra app davanti: notifica «Download finito: N scaricati» e icona che saltella.
- [manual] Il titolo della finestra è «Grabbit — 45%» durante il download, «Grabbit» alla fine.
Stato: fatta

### S-008 Aggiungere il link appena copiato
Come utente, quando torno su Grabbit dopo aver copiato un link YouTube o Spotify, Grabbit
me lo propone con «Aggiungi» e «No, grazie».
Accettazione:
- [unit] Si propone solo un link YouTube, YouTube Music o Spotify, da solo; non un testo qualsiasi.
- [snap] L'avviso compare nel riquadro «Link»; «Aggiungi» mette il link nel campo, «No, grazie» lo chiude.
Stato: fatta

### S-009 Vedere le copertine senza deformazioni
Come utente, in «Ascolta» le copertine sono quadrate, senza bande e non schiacciate.
Accettazione:
- [unit] Una copertina 16:9 di YouTube diventa il quadrato centrale.
- [unit] Per l'audio yt-dlp salva nei file la copertina già quadrata (filtro crop di ffmpeg).
- [manual] Sul Mac «25» e «19» di Adele hanno la copertina giusta.
Stato: fatta

## Milestone 4: libreria e player più comodi (0.6.0)

S-010, S-011

### S-010 Vedere nei risultati cosa ho già
Come utente, nei risultati della ricerca gli album e i brani che ho già nella libreria sono
segnati «Già nella libreria», con «Ascolta» che li apre nella scheda Ascolta.
Accettazione:
- [unit] Un risultato si ritrova nella libreria per titolo e artista, senza badare a maiuscole, punteggiatura o un anno davanti alla cartella («2002.Tracce»).
- [snap] Il brano che c'è mostra «Già nella libreria · Ascolta», quello che manca no.
- [manual] Sul Mac, nella discografia di un artista gli album già scaricati sono segnati.
Stato: fatta

### S-011 Ascoltare in modo più comodo
Come utente, nel player ho «Casuale» e «Ripeti» (tutto o il brano), la barra spaziatrice e
le frecce, «Ascolta tutto» per un artista, e alla riapertura ritrovo il brano dove l'avevo lasciato.
Accettazione:
- [unit] Il mescolamento tiene tutti i brani una volta sola.
- [snap] Barra del player con «Casuale» e «Ripeti brano» attivi, leggibili; «Ascolta tutto (N brani)» sugli artisti con più uscite.
- [snap] Dopo un riavvio la barra mostra l'ultimo brano in pausa alla posizione salvata, con le stesse scelte di Casuale e Ripeti.
- [manual] Sul Mac: spazio mette in pausa, le frecce cambiano brano, «Ripeti tutto» ricomincia l'album.
Stato: fatta

## Milestone 5: libreria e ascolto curati (0.7.0)

S-012, S-013, S-014

### S-012 Gestire un album da «Ascolta»
Come utente, nella pagina di un album ho «Mostra nel Finder», «Aggiungi a Musica» (iTunes sui
Mac meno recenti) e «Sposta nel Cestino…», che chiede conferma.
Accettazione:
- [snap] I pulsanti sotto «Ascolta l'album»; niente Cestino per i «Brani sciolti».
- [snap] La conferma nomina l'album; dopo, la cartella è nel Cestino e l'elenco si aggiorna con «“…” è nel Cestino.».
- [manual] Sul Mac: «Aggiungi a iTunes» importa l'album; dal Cestino «Ripristina» lo rimette a posto.
Stato: fatta

### S-013 Scegliere i brani di un album prima di scaricarlo
Come utente, negli album dei risultati premo «Scegli i brani…», tolgo la spunta a quelli che
non voglio e scarico solo gli altri, con il loro numero nell'album.
Accettazione:
- [unit] Solo i brani scelti si scaricano, come «01 - …» e «03 - …».
- [snap] L'elenco con le caselle, «Tutti», «Nessuno» e «Scarica i brani scelti (N)».
- [manual] Sul Mac, con un album vero di YouTube Music.
Stato: fatta

### S-014 Ascoltare tutti i brani allo stesso volume
Come utente, con «Volume uguale tra i brani» (attivo di serie) i brani di album diversi
suonano allo stesso volume, senza che i file vengano toccati.
Accettazione:
- [unit] Un brano forte si abbassa, uno piano si alza (al massimo 6 dB e mai oltre il picco); i silenzi non contano.
- [unit] Un file vero si misura e la misura resta nell'archivio.
- [snap] La casella in «Ascolta»; i brani della libreria si misurano in sottofondo.
- [manual] Sul Mac, passando da un album vecchio a uno recente il volume resta simile.
Stato: fatta

### S-015 Guardare un video musicale dalla libreria
Come utente, un video musicale scaricato compare in «Ascolta» segnato «Video» e, cliccandolo,
si apre nel player video del computer (QuickTime sul Mac).
Accettazione:
- [unit] Un file video (MP4 con una traccia video, MKV, MOV, M4V) è segnato come video; la durata viene dalla traccia audio.
- [snap] La riga mostra «Video» e la durata; «Ascolta l'album», Casuale e «Ascolta tutto» saltano i video.
- [manual] Sul Mac il video si apre in QuickTime e la musica di Grabbit si mette in pausa.
Stato: fatta
