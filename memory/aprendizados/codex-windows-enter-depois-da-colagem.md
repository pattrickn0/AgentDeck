---
titulo: No Windows o Codex engole o Enter que chega antes de terminar de ler a colagem
resumo: O Codex lê a colagem tecla por tecla; o Enter cedo vira quebra de linha. O atraso do Enter cresce 1 ms por caractere (delivery::enter_delay).
touches: [crates/core/src/delivery.rs, crates/app/src/deck.rs]
---
O app cola as mensagens com bracketed paste e manda o Enter 250 ms depois. No Windows o Codex 0.164 liga o modo de
colagem (`?2004h`), mas pelo ConPTY recebe o texto como teclas soltas e o digere aos poucos: o Enter que chega antes do
fim vira uma quebra de linha dentro do "[Pasted Content N chars]" e a mensagem fica parada no campo até alguém apertar
Enter. Medido em 2026-10-25 com uma sonda de PTY: 1.735 caracteres perdem o Enter em 600 ms e pegam em 1.200 ms; 3.593
perdem em 1.200 e pegam em 2.400. Texto curto pega em 250 ms.

**Por quê:** o usuário precisava apertar Enter à mão nas tarefas do Codex (o pedido e as mensagens do `ask-agent` são longos).
**Como aplicar:** o atraso do primeiro Enter vem de `delivery::enter_delay` (só Codex no Windows, 1 ms por caractere, teto de
10 s). Não "resolva" com um segundo Enter fixo: num Codex que acabou de abrir uma pergunta de aprovação, o Enter a mais
aprova o comando. CLI novo que deixe a mensagem parada no campo: meça do mesmo jeito (colar N caracteres e variar o atraso).
