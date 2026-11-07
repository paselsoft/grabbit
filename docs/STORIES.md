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
- [snap] Durante un download la barra mostra «Annulla», una barra di avanzamento e «X di N completati».
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
Stato: in corso

### S-003 Salvare la musica in cartelle Artista / Album
Come utente, quando scarico audio i brani vanno in `Artista/Album/01 - Titolo.m4a`,
come in iTunes e Plex; posso spegnerlo in «Opzioni avanzate».
Accettazione:
- [unit] Un album di YouTube Music (playlist OLAK5uy_) va in «Artista/Album/01 - Titolo», con l'artista senza « - Topic».
- [unit] Un brano singolo usa i metadati di yt-dlp: «artista/album/titolo», con «Singoli» se manca l'album.
- [unit] Una playlist che non è un album resta nella cartella con il nome della playlist; i video e l'opzione spenta restano come prima.
- [snap] «Ascolta» mostra gli album dentro le cartelle degli artisti.
- [manual] Sul Mac: scarica un album e un brano da YouTube Music e controlla le cartelle.
UX:
- Posto: casella in «Opzioni avanzate», attiva di serie
- Termini nuovi: Cartelle Artista / Album (Artist / Album folders), Singoli (Singles)
Stato: in corso
