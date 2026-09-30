# Progress - 2026-09-30

## Status

Prototipul Python functioneaza bine ca demonstratie conceptuala:
- porneste QEMU headless
- citeste QMP
- face screendump pe interval regulat
- trimite input de tastatura si text catre DOS
- ruleaza local intr-un Tk viewer

Conceptul este valid si realist pentru varianta Rust.

## Ce facem acum

### 1. MVP Rust minim

Scopul este sa recream nivelul minim de functionalitate de la Python, dar in Rust:
- pornire QEMU in headless mode
- conectare la QMP socket
- comenzi de baza: `qmp_capabilities`, `screendump`, `human-monitor-command`, `input-send-event`
- afisare preview local a frame-ului
- tastatura de baza: letters, numbers, space, enter, backspace, arrows

### 2. Ce trebuie sa functioneze pentru un milestone bun

Pana la primul milestone, trebuie sa se poata:
- booteze DOS in QEMU
- sa se vada ecranul
- sa se trimita input real de tastatura
- sa se testeze interactivitatea cu un shell sau un program DOS simplu

Acesta este criteriul principal de succes.

## Arhitectura de lucru pentru MVP

### Core modules

- `vm` : pornire QEMU, QMP connection
- `qmp` : JSON protocol, command execution, async handling
- `video` : screendump, frame buffering, local preview
- `input` : keyboard mapping, key-down/key-up, basic events
- `ui` : preview local / test window

### Important

Nu mergem direct la browser streaming. 
Primul pas este sa avem un client local care sa demonstreze ca:
- DOS boot-eaza
- frame-ul poate fi capturat
- input-ul functioneaza

Dupa ce asta e stabil, se poate extinde la browser, audio si protocol web.

## Decizii de design

### Video

Pentru MVP folosim o varianta echivalenta cu Python:
- poll `screendump`
- salvare frame in PPM sau raw
- preview local in un widget grafic

Aceasta este o solutie de validare, nu finala.

### Input

Pentru tastatura de baza:
- mapam keycodes / `keysym` in qcodes QEMU
- trimitem `input-send-event` cu `down: true/false`
- pentru text simplu folosim `human-monitor-command sendkey ...` ca fallback

### Audio

Nu este prioritizat pentru MVP. 
Audio-ul este o functionalitate ulterioara si nu trebuie sa blocheze primul milestone.

## Next actions

1. crea un proiect Rust minim
2. implementare QMP client cu comenzi de baza
3. start QEMU in headless mode
4. test `screendump`
5. implementare keyboard basics
6. validate cu un DOS shell / program simplu

## Observatie finala

MVP-ul Rust nu trebuie sa fie complet din punct de vedere browser/web. 
Trebuie doar sa valideze conceptul principal: DOS bootabil cu input si preview.
