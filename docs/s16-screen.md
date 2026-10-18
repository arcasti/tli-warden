# The S16 prison screen

Notes for the screenshot reader, from one recorded run (trainingdata/s16/rec1-tNNN.png = trainingdata/rec-2026-10-16/fps1/tNNN.png, 1918x1078, one frame per second). Labels: data/scan-truth-s16.json.

## Rarity colours (glow around the portrait and on the destination card)
white/grey = 0 Normal, blue = 1 Magic, purple = 2 Rare, orange/gold = 3 Legendary, red = 4 Ultimate.
Confidence: high for blue -> purple -> orange (t071-t075: a blue eats a blue and turns purple, then eats a purple and turns orange) and for red = Ultimate (t081-t083: an orange eats an orange, then eats the red cell 4 and the Chromatic popup follows; the objective text writes "Ultimate" in red). Medium for white = Normal: white glows appear on fresh inmates, but lantern light near the warden also whitens glows.
Chromatic: rainbow/green-pink flash on the cell (t125), then the portrait moves to the Chromatic Prisoner panel with a pink-green flame.

## Beasts
Checked on 120x130 portrait crops of every labeled frame:
- Colossal Rat: pale pink face, big round ears, long snout.
- Alpha Wolf: dark purple fur, green eyes, pointed ears.
- Berserk Lion: tan face in a full pale mane, yellow eyes.
- Mammoth: pale grey heavy brow over small purple eyes, wide jaw.
The food chain confirms the reading: a wolf eats a rat (t049-t059), a lion eats a rat (t067), a lion eats a wolf (t071), a rat eats a mammoth (t077), a mammoth eats a lion (t113).
A Consume re-rolls the eater's beast: cell 7 is a wolf before eating cell 5 and a rat after (t059).
The warden's lantern lights its cell amber and hides a Normal glow (t089 cell 10, t097 cell 6); read those tiers from the card art or the frame instead.
A small red jaws crown on top of a portrait (t049 cell 7) is the jaws marker: that Inmate has something to consume.

## Screen types and where things are (pixels at 1918x1078)
- Board area: x 420-1530, y 75-680; courtyard x 640-1290, y 265-495.
- Portrait centres: top row cells 1-6 at y 140, x 520, 698, 871, 1045, 1220, 1395; cell 7 (1400, 270), cell 8 (1405, 420); bottom row y 560, cells 9-14 at x 1416, 1233, 1052, 869, 686, 504; cell 16 (515, 280), cell 15 (512, 420). Portrait box about 70x90 around the centre; cell box about 150x120.
- Cell numbers: top row at y 52-70 above each cell (1 at 538, 52); 7 and 8 at x 1495; bottom row at y 653; 15 and 16 at x 418.
- Padlock: bottom middle of the cell box: top row y 195, bottom row y 620, cell 8 (1407, 470), cell 16 (515, 332), cell 15 (512, 473).
- Prison Incident "!": orange, at the cell's outer bottom-right (cell 9: 1487, 593).
- Warden arrow (gold): just inside the ring next to its cell, pointing at it; top row cells below the cell (y 230), bottom row above (y 460), side cells beside it (cell 15: 610, 420); corner cells (1, 6, 9, 14) get a diagonal arrow at the inner corner (cell 1: 615, 220; cell 6: about 1290, 220; cell 14: about 610, 470).
- Days counter: "N day(s) until Riot", x 830-1090, y 10-50; the number turns red at 3 and below.
- Destination cards: "Select a Destination" at y 697; three cards x 450-780, 795-1125, 1140-1470, y 750-920. Card art: closed door = locked cell, beast portrait with rarity glow = open cell, lit room = Go to Interrogation Room. A card for a cell with an incident shows a small "!" at its top right (t087, t115).
- Decision buttons: "Make Your Decision" at y 697, same row as the cards: Consume (red jaws), Replace (arrows), End (gear); with two buttons they are centred (Replace/End t113, Consume/End t123).
- Consume target pick: whole screen tinted red, red banner "Select an Inmate to Consume" at about (960, 830), Cancel button (1720, 800). Hovered target gets big red jaws around its portrait. A chain shows "Chain Consume" over the eater (t073).
- Instrument panel: heading "Torture Instrument" (80-280, 297); icons below from (122, 392), one per instrument.
- Chromatic panel: heading at (1600-1815, 157); two columns of hexagon slots x 1690-1840, y 190-670; first slots at about (1720, 230) and (1805, 230).
- Objective panel: x 30-380, y 135-245 ("Objective: Undercurrents / Trigger ! Prison Incident 2 times 0/2").
- Instrument pick (t031-t045): "Select a Torture Instrument", three cards x 250-1180, y 140-860, Confirm (960, 960). Hovered card gets an orange frame.
- Warden's Treasure pick (t085, t127): "Prisoner rarity reached Chromatic. Warden's Treasure obtained.", four cards x 100-1840, y 150-830, Confirm (960, 960), "View Prison Status" eye at top right.
- Day banner (t095, t103, t135): "Day N" plaque at the board centre (x 490-1250, y 400-500), board dimmed. Day N = 20 - days until Riot.
- Other: Speed-up (1540-1705, 20-60), Quick Action toggle (1725, 950-1005), objective popup before the run (t025-t027).

## Destinations
Each morning shows one card per die, nearest first; the Interrogation Room card appears only when a die lands on the room, in its distance order (data/scan-truth-s16.json, field `doors`, 1 = the room):
- t063, warden at 12: cards 13, 15, room (1, 3, 5 steps).
- t131, warden at 15: cards room, 3, 5 (2, 4, 6 steps).
- t049, t069, t087, t115, t167: three cell cards, no room card.

Both engines follow this: today's destinations are the distinct cells the dice land on, the room included only when a die lands there, sorted by steps from the warden.

## Markers not in the rules list
- Cells highlighted pale (lighter stone) with thin connecting lines: the reachable cells of the current destination options or the move path (t047, t063, t093).
- A small white "Y"/sparkle icon in cell 9 next to the "!" (t087); meaning unknown.
- Run start: the board shows all 15 cells locked until the instrument is picked; the 4 opened cells appear after (t029 vs t047).

## Run timeline (days until Riot; Day = 20 - that)
- 20: objective Undercurrents (Prison Incident 2 times); picks Kin's Coin of Fate over Ravenous Chains and Scapegoat's Guillotine. Incident marker at 9 the whole run.
- 19: open 4 rat/4, 5 rat/1, 6 rat/3, 7 wolf/1. Goes to 7; 7 eats 5 (wolf/2).
- 18: to 12 (opens blue). 17: cards 13/15/Interrogation Room; to 15, 15 eats 12. Cards 16/3/5.
- 16: to 5 (new blue); chain: 5 eats 15 then 7, ends orange.
- 15: 6 eats 5 (Ultimate), then eats 4 (Ultimate): Chromatic 1. Treasure: Three-Headed Thumb Ring (opens 7, 11, 13).
- 14: to 10; 10 eats 13. 12: Day 8; warden at 6; 6 and 16 open. 11-10: to 12 and 16; 16 eats 12.
- 9: to 5, Replace. 8: cards 6/9/10; Consume at 10. 7: at 15, Consume; cell 7 (orange) reaches Chromatic: 2. Thumb Ring again (opens 2, 3, 8).
- 6-5: Day 14; to 5, 5 eats 2. 4: to 14 (opens), Replace/End. 3: chain at 4 (3 and 4 flash, target 16).
- 2: to 8 (target 14). 1: to 10, 10 reaches Ultimate (cell 4 gone). 0: to 16, Consume/Replace/End, then the Riot.
- No Prison Incident fired (objective 0/2) and the Interrogation Room was never entered.
