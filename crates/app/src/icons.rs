//! Ícones SVG da interface original, rasterizados em tempo de execução a partir dos
//! mesmos `path d="…"` do HTML/JS (sem arquivos de imagem). O rasterizador é um
//! preenchimento por varredura com regra "nonzero" e cobertura horizontal exata
//! (anti-aliasing), suficiente para ícones pequenos e nítidos em qualquer escala.

use std::collections::HashMap;

use egui::{Color32, Mesh, Pos2, Rect, TextureHandle, TextureOptions, Vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    /// Ícone do estado vazio (viewBox 24).
    TerminalBox,
    /// `#mic-cancel` (viewBox 24).
    Close,
    /// `#mic` (viewBox 24).
    Mic,
    /// `#attach` (viewBox 24).
    Attach,
    /// Nuvem do overlay de arrastar e soltar (viewBox 24).
    CloudUpload,
    /// Chevron da árvore de arquivos (viewBox 16).
    TreeChevron,
    /// Pasta aberta (viewBox 16).
    FolderOpen,
    /// Pasta fechada (viewBox 16).
    FolderClosed,
    /// Arquivo genérico (viewBox 16).
    FileGeneric,
    /// Aba "Conversa integrada": Material Icons `forum` (Apache-2.0, viewBox 24).
    Chat,
    /// Aba "Mapa de agentes": Material Icons `account_tree` (Apache-2.0, viewBox 24).
    Diagram,
    /// Aba "Git" e chip da branch: Octicons `git-branch-16` (MIT, viewBox 16).
    GitBranch,
    /// Aba "GitHub": Octicons `mark-github-16` (MIT, viewBox 16).
    GitHub,
    /// Git: buscar (fetch). Material Icons `refresh` (Apache-2.0, viewBox 24).
    Refresh,
    /// Git: puxar (pull). Material Icons `arrow_downward` (Apache-2.0, viewBox 24).
    ArrowDown,
    /// Git: enviar (push). Material Icons `arrow_upward` (Apache-2.0, viewBox 24).
    ArrowUp,
    /// Git: descartar alterações. Material Icons `undo` (Apache-2.0, viewBox 24).
    Undo,
    /// Barra lateral (ORGANIZAÇÃO): liga/desliga a tela dividida. Material Icons `space_dashboard` (Apache-2.0, viewBox 24).
    Split,
    /// Logo do Claude: o "spark", o asterisco laranja (viewBox 24).
    ClaudeLogo,
    /// Logo do Google Antigravity (viewBox 112).
    AntigravityLogo,
    /// Logo do Ollama: a lhama (viewBox 24).
    OllamaLogo,
    /// Logo do Codex / OpenAI (viewBox 24).
    CodexLogo,
}

impl Icon {
    fn path(self) -> (&'static str, f32) {
        match self {
            Icon::TerminalBox => ("M20 4H4c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V6c0-1.1-.9-2-2-2zm0 14H4V8h16v10zm-2-1h-4v-2h4v2zm-6.29-2.71a.996.996 0 0 0 0-1.41L9.42 11.6l2.3-2.3a.996.996 0 1 0-1.41-1.41L7.59 10.6c-.39.39-.39 1.02 0 1.41l2.71 2.71c.39.39 1.02.39 1.41-.01z", 24.0),
            Icon::Close => ("M19 6.41L17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12 19 6.41z", 24.0),
            Icon::Mic => ("M12 14a3 3 0 0 0 3-3V5a3 3 0 0 0-6 0v6a3 3 0 0 0 3 3Zm5-3a5 5 0 0 1-10 0H5a7 7 0 0 0 6 6.92V21h2v-3.08A7 7 0 0 0 19 11h-2Z", 24.0),
            Icon::Attach => ("M16.5 6.5v10a4.5 4.5 0 0 1-9 0V5a3 3 0 0 1 6 0v10.5a1.5 1.5 0 0 1-3 0V6.5H9v9a3 3 0 0 0 6 0V5a4.5 4.5 0 0 0-9 0v11.5a6 6 0 0 0 12 0v-10h-1.5Z", 24.0),
            Icon::CloudUpload => ("M19.35 10.04C18.67 6.59 15.64 4 12 4 9.11 4 6.6 5.64 5.35 8.04 2.34 8.36 0 10.91 0 14c0 3.31 2.69 6 6 6h13c2.76 0 5-2.24 5-5 0-2.64-2.05-4.78-4.65-4.96zM14 13v4h-4v-4H7l5-5 5 5h-3z", 24.0),
            Icon::TreeChevron => ("M6.22 3.22a.75.75 0 0 1 1.06 0l4.25 4.25a.75.75 0 0 1 0 1.06l-4.25 4.25a.75.75 0 0 1-1.06-1.06L9.94 8 6.22 4.28a.75.75 0 0 1 0-1.06z", 16.0),
            Icon::FolderOpen => ("M1.5 2A1.5 1.5 0 0 0 0 3.5v9A1.5 1.5 0 0 0 1.5 14h13a1.5 1.5 0 0 0 1.5-1.5v-7A1.5 1.5 0 0 0 14.5 4H7.414L5.707 2.293A1 1 0 0 0 5 2H1.5zm0 1H5l1.707 1.707A1 1 0 0 0 7.414 5H14.5a.5.5 0 0 1 .5.5v1.25H1V3.5a.5.5 0 0 1 .5-.5z", 16.0),
            Icon::FolderClosed => ("M14.5 3H7.71l-.85-.85L6.51 2h-5l-.5.5v11l.5.5h13l.5-.5v-10l-.5-.5zm-.5 10H2V3h4.29l.86.85.35.15H14v9z", 16.0),
            Icon::FileGeneric => ("M9 1H3a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V7L9 1zm4 12H3V3h5v5h5v5z", 16.0),
            Icon::Chat => ("M21 6h-2v9H6v2c0 .55.45 1 1 1h11l4 4V7c0-.55-.45-1-1-1zm-4 6V3c0-.55-.45-1-1-1H3c-.55 0-1 .45-1 1v14l4-4h10c.55 0 1-.45 1-1z", 24.0),
            Icon::Diagram => ("M22 11V3h-7v3H9V3H2v8h7V8h2v10h4v3h7v-8h-7v3h-2V8h2v3z", 24.0),
            Icon::GitBranch => ("M9.5 3.25a2.25 2.25 0 1 1 3 2.122V6A2.5 2.5 0 0 1 10 8.5H6a1 1 0 0 0-1 1v1.128a2.251 2.251 0 1 1-1.5 0V5.372a2.25 2.25 0 1 1 1.5 0v1.836A2.493 2.493 0 0 1 6 7h4a1 1 0 0 0 1-1v-.628A2.25 2.25 0 0 1 9.5 3.25Zm-6 0a.75.75 0 1 0 1.5 0 .75.75 0 0 0-1.5 0Zm8.25-.75a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5ZM4.25 12a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5Z", 16.0),
            Icon::GitHub => ("M8 0c4.42 0 8 3.58 8 8a8.013 8.013 0 0 1-5.45 7.59c-.4.08-.55-.17-.55-.38 0-.27.01-1.13.01-2.2 0-.75-.25-1.23-.54-1.48 1.78-.2 3.65-.88 3.65-3.95 0-.88-.31-1.59-.82-2.15.08-.2.36-1.02-.08-2.12 0 0-.67-.22-2.2.82-.64-.18-1.32-.27-2-.27-.68 0-1.36.09-2 .27-1.53-1.03-2.2-.82-2.2-.82-.44 1.1-.16 1.92-.08 2.12-.51.56-.82 1.28-.82 2.15 0 3.06 1.86 3.75 3.64 3.95-.23.2-.44.55-.51 1.07-.46.21-1.61.55-2.33-.66-.15-.24-.6-.83-1.23-.82-.67.01-.27.38.01.53.34.19.73.9.82 1.13.16.45.68 1.31 2.69.94 0 .67.01 1.3.01 1.49 0 .21-.15.45-.55.38A7.995 7.995 0 0 1 0 8c0-4.42 3.58-8 8-8Z", 16.0),
            Icon::Refresh => ("M17.65 6.35A7.958 7.958 0 0 0 12 4c-4.42 0-7.99 3.58-7.99 8s3.57 8 7.99 8c3.73 0 6.84-2.55 7.73-6h-2.08A5.99 5.99 0 0 1 12 18c-3.31 0-6-2.69-6-6s2.69-6 6-6c1.66 0 3.14.69 4.22 1.78L13 11h7V4l-2.35 2.35z", 24.0),
            Icon::Split => ("M11 21H5c-1.1 0-2-.9-2-2V5c0-1.1.9-2 2-2h6v18zm2 0h6c1.1 0 2-.9 2-2v-7h-8v9zm8-11V5c0-1.1-.9-2-2-2h-6v7h8z", 24.0),
            Icon::ArrowDown => ("m20 12-1.41-1.41L13 16.17V4h-2v12.17l-5.58-5.59L4 12l8 8 8-8z", 24.0),
            Icon::ArrowUp => ("m4 12 1.41 1.41L11 7.83V20h2V7.83l5.58 5.59L20 12l-8-8-8 8z", 24.0),
            Icon::Undo => ("M12.5 8c-2.65 0-5.05.99-6.9 2.6L2 7v9h9l-3.62-3.62c1.39-1.16 3.16-1.88 5.12-1.88 3.54 0 6.55 2.31 7.6 5.5l2.37-.78C21.08 11.03 17.15 8 12.5 8z", 24.0),
            Icon::ClaudeLogo => ("m4.7144 15.9555 4.7174-2.6471.079-.2307-.079-.1275h-.2307l-.7893-.0486-2.6956-.0729-2.3375-.0971-2.2646-.1214-.5707-.1215-.5343-.7042.0546-.3522.4797-.3218.686.0608 1.5179.1032 2.2767.1578 1.6514.0972 2.4468.255h.3886l.0546-.1579-.1336-.0971-.1032-.0972L6.973 9.8356l-2.55-1.6879-1.3356-.9714-.7225-.4918-.3643-.4614-.1578-1.0078.6557-.7225.8803.0607.2246.0607.8925.686 1.9064 1.4754 2.4893 1.8336.3643.3035.1457-.1032.0182-.0728-.164-.2733-1.3539-2.4467-1.445-2.4893-.6435-1.032-.17-.6194c-.0607-.255-.1032-.4674-.1032-.7285L6.287.1335 6.6997 0l.9957.1336.419.3642.6192 1.4147 1.0018 2.2282 1.5543 3.0296.4553.8985.2429.8318.091.255h.1579v-.1457l.1275-1.706.2368-2.0947.2307-2.6957.0789-.7589.3764-.9107.7468-.4918.5828.2793.4797.686-.0668.4433-.2853 1.8517-.5586 2.9021-.3643 1.9429h.2125l.2429-.2429.9835-1.3053 1.6514-2.0643.7286-.8196.85-.9046.5464-.4311h1.0321l.759 1.1293-.34 1.1657-1.0625 1.3478-.8804 1.1414-1.2628 1.7-.7893 1.36.0729.1093.1882-.0183 2.8535-.607 1.5421-.2794 1.8396-.3157.8318.3886.091.3946-.3278.8075-1.967.4857-2.3072.4614-3.4364.8136-.0425.0304.0486.0607 1.5482.1457.6618.0364h1.621l3.0175.2247.7892.522.4736.6376-.079.4857-1.2142.6193-1.6393-.3886-3.825-.9107-1.3113-.3279h-.1822v.1093l1.0929 1.0686 2.0035 1.8092 2.5075 2.3314.1275.5768-.3218.4554-.34-.0486-2.2039-1.6575-.85-.7468-1.9246-1.621h-.1275v.17l.4432.6496 2.3436 3.5214.1214 1.0807-.17.3521-.6071.2125-.6679-.1214-1.3721-1.9246L14.38 17.959l-1.1414-1.9428-.1397.079-.674 7.2552-.3156.3703-.7286.2793-.6071-.4614-.3218-.7468.3218-1.4753.3886-1.9246.3157-1.53.2853-1.9004.17-.6314-.0121-.0425-.1397.0182-1.4328 1.9672-2.1796 2.9446-1.7243 1.8456-.4128.164-.7164-.3704.0667-.6618.4008-.5889 2.386-3.0357 1.4389-1.882.929-1.0868-.0062-.1579h-.0546l-6.3385 4.1164-1.1293.1457-.4857-.4554.0608-.7467.2307-.2429 1.9064-1.3114Z", 24.0),
            Icon::AntigravityLogo => ("M89.6992 93.695C94.3659 97.195 101.366 94.8617 94.9492 88.445C75.6992 69.7783 79.7825 18.445 55.8659 18.445C31.9492 18.445 36.0325 69.7783 16.7825 88.445C9.78251 95.445 17.3658 97.195 22.0325 93.695C40.1159 81.445 38.9492 59.8617 55.8659 59.8617C72.7825 59.8617 71.6159 81.445 89.6992 93.695Z", 112.0),
            Icon::OllamaLogo => ("M16.361 10.26a.894.894 0 0 0-.558.47l-.072.148.001.207c0 .193.004.217.059.353.076.193.152.312.291.448.24.238.51.3.872.205a.86.86 0 0 0 .517-.436.752.752 0 0 0 .08-.498c-.064-.453-.33-.782-.724-.897a1.06 1.06 0 0 0-.466 0zm-9.203.005c-.305.096-.533.32-.65.639a1.187 1.187 0 0 0-.06.52c.057.309.31.59.598.667.362.095.632.033.872-.205.14-.136.215-.255.291-.448.055-.136.059-.16.059-.353l.001-.207-.072-.148a.894.894 0 0 0-.565-.472 1.02 1.02 0 0 0-.474.007Zm4.184 2c-.131.071-.223.25-.195.383.031.143.157.288.353.407.105.063.112.072.117.136.004.038-.01.146-.029.243-.02.094-.036.194-.036.222.002.074.07.195.143.253.064.052.076.054.255.059.164.005.198.001.264-.03.169-.082.212-.234.15-.525-.052-.243-.042-.28.087-.355.137-.08.281-.219.324-.314a.365.365 0 0 0-.175-.48.394.394 0 0 0-.181-.033c-.126 0-.207.03-.355.124l-.085.053-.053-.032c-.219-.13-.259-.145-.391-.143a.396.396 0 0 0-.193.032zm.39-2.195c-.373.036-.475.05-.654.086-.291.06-.68.195-.951.328-.94.46-1.589 1.226-1.787 2.114-.04.176-.045.234-.045.53 0 .294.005.357.043.524.264 1.16 1.332 2.017 2.714 2.173.3.033 1.596.033 1.896 0 1.11-.125 2.064-.727 2.493-1.571.114-.226.169-.372.22-.602.039-.167.044-.23.044-.523 0-.297-.005-.355-.045-.531-.288-1.29-1.539-2.304-3.072-2.497a6.873 6.873 0 0 0-.855-.031zm.645.937a3.283 3.283 0 0 1 1.44.514c.223.148.537.458.671.662.166.251.26.508.303.82.02.143.01.251-.043.482-.08.345-.332.705-.672.957a3.115 3.115 0 0 1-.689.348c-.382.122-.632.144-1.525.138-.582-.006-.686-.01-.853-.042-.57-.107-1.022-.334-1.35-.68-.264-.28-.385-.535-.45-.946-.03-.192.025-.509.137-.776.136-.326.488-.73.836-.963.403-.269.934-.46 1.422-.512.187-.02.586-.02.773-.002zm-5.503-11a1.653 1.653 0 0 0-.683.298C5.617.74 5.173 1.666 4.985 2.819c-.07.436-.119 1.04-.119 1.503 0 .544.064 1.24.155 1.721.02.107.031.202.023.208a8.12 8.12 0 0 1-.187.152 5.324 5.324 0 0 0-.949 1.02 5.49 5.49 0 0 0-.94 2.339 6.625 6.625 0 0 0-.023 1.357c.091.78.325 1.438.727 2.04l.13.195-.037.064c-.269.452-.498 1.105-.605 1.732-.084.496-.095.629-.095 1.294 0 .67.009.803.088 1.266.095.555.288 1.143.503 1.534.071.128.243.393.264.407.007.003-.014.067-.046.141a7.405 7.405 0 0 0-.548 1.873c-.062.417-.071.552-.071.991 0 .56.031.832.148 1.279L3.42 24h1.478l-.05-.091c-.297-.552-.325-1.575-.068-2.597.117-.472.25-.819.498-1.296l.148-.29v-.177c0-.165-.003-.184-.057-.293a.915.915 0 0 0-.194-.25 1.74 1.74 0 0 1-.385-.543c-.424-.92-.506-2.286-.208-3.451.124-.486.329-.918.544-1.154a.787.787 0 0 0 .223-.531c0-.195-.07-.355-.224-.522a3.136 3.136 0 0 1-.817-1.729c-.14-.96.114-2.005.69-2.834.563-.814 1.353-1.336 2.237-1.475.199-.033.57-.028.776.01.226.04.367.028.512-.041.179-.085.268-.19.374-.431.093-.215.165-.333.36-.576.234-.29.46-.489.822-.729.413-.27.884-.467 1.352-.561.17-.035.25-.04.569-.04.319 0 .398.005.569.04a4.07 4.07 0 0 1 1.914.997c.117.109.398.457.488.602.034.057.095.177.132.267.105.241.195.346.374.43.14.068.286.082.503.045.343-.058.607-.053.943.016 1.144.23 2.14 1.173 2.581 2.437.385 1.108.276 2.267-.296 3.153-.097.15-.193.27-.333.419-.301.322-.301.722-.001 1.053.493.539.801 1.866.708 3.036-.062.772-.26 1.463-.533 1.854a2.096 2.096 0 0 1-.224.258.916.916 0 0 0-.194.25c-.054.109-.057.128-.057.293v.178l.148.29c.248.476.38.823.498 1.295.253 1.008.231 2.01-.059 2.581a.845.845 0 0 0-.044.098c0 .006.329.009.732.009h.73l.02-.074.036-.134c.019-.076.057-.3.088-.516.029-.217.029-1.016 0-1.258-.11-.875-.295-1.57-.597-2.226-.032-.074-.053-.138-.046-.141.008-.005.057-.074.108-.152.376-.569.607-1.284.724-2.228.031-.26.031-1.378 0-1.628-.083-.645-.182-1.082-.348-1.525a6.083 6.083 0 0 0-.329-.7l-.038-.064.131-.194c.402-.604.636-1.262.727-2.04a6.625 6.625 0 0 0-.024-1.358 5.512 5.512 0 0 0-.939-2.339 5.325 5.325 0 0 0-.95-1.02 8.097 8.097 0 0 1-.186-.152.692.692 0 0 1 .023-.208c.208-1.087.201-2.443-.017-3.503-.19-.924-.535-1.658-.98-2.082-.354-.338-.716-.482-1.15-.455-.996.059-1.8 1.205-2.116 3.01a6.805 6.805 0 0 0-.097.726c0 .036-.007.066-.015.066a.96.96 0 0 1-.149-.078A4.857 4.857 0 0 0 12 3.03c-.832 0-1.687.243-2.456.698a.958.958 0 0 1-.148.078c-.008 0-.015-.03-.015-.066a6.71 6.71 0 0 0-.097-.725C8.997 1.392 8.337.319 7.46.048a2.096 2.096 0 0 0-.585-.041Zm.293 1.402c.248.197.523.759.682 1.388.03.113.06.244.069.292.007.047.026.152.041.233.067.365.098.76.102 1.24l.002.475-.12.175-.118.178h-.278c-.324 0-.646.041-.954.124l-.238.06c-.033.007-.038-.003-.057-.144a8.438 8.438 0 0 1 .016-2.323c.124-.788.413-1.501.696-1.711.067-.05.079-.049.157.013zm9.825-.012c.17.126.358.46.498.888.28.854.36 2.028.212 3.145-.019.14-.024.151-.057.144l-.238-.06a3.693 3.693 0 0 0-.954-.124h-.278l-.119-.178-.119-.175.002-.474c.004-.669.066-1.19.214-1.772.157-.623.434-1.185.68-1.382.078-.062.09-.063.159-.012z", 24.0),
            Icon::CodexLogo => ("M22.2819 9.8211a5.9847 5.9847 0 0 0-.5157-4.9108 6.0462 6.0462 0 0 0-6.5098-2.9A6.0651 6.0651 0 0 0 4.9807 4.1818a5.9847 5.9847 0 0 0-3.9977 2.9 6.0462 6.0462 0 0 0 .7427 7.0966 5.98 5.98 0 0 0 .511 4.9107 6.051 6.051 0 0 0 6.5146 2.9001A5.9847 5.9847 0 0 0 13.2599 24a6.0557 6.0557 0 0 0 5.7718-4.2058 5.9894 5.9894 0 0 0 3.9977-2.9001 6.0557 6.0557 0 0 0-.7475-7.0729zm-9.022 12.6081a4.4755 4.4755 0 0 1-2.8764-1.0408l.1419-.0804 4.7783-2.7582a.7948.7948 0 0 0 .3927-.6813v-6.7369l2.02 1.1686a.071.071 0 0 1 .038.052v5.5826a4.504 4.504 0 0 1-4.4945 4.4944zm-9.6607-4.1254a4.4708 4.4708 0 0 1-.5346-3.0137l.142.0852 4.783 2.7582a.7712.7712 0 0 0 .7806 0l5.8428-3.3685v2.3324a.0804.0804 0 0 1-.0332.0615L9.74 19.9502a4.4992 4.4992 0 0 1-6.1408-1.6464zM2.3408 7.8956a4.485 4.485 0 0 1 2.3655-1.9728V11.6a.7664.7664 0 0 0 .3879.6765l5.8144 3.3543-2.0201 1.1685a.0757.0757 0 0 1-.071 0l-4.8303-2.7865A4.504 4.504 0 0 1 2.3408 7.872zm16.5963 3.8558L13.1038 8.364 15.1192 7.2a.0757.0757 0 0 1 .071 0l4.8303 2.7913a4.4944 4.4944 0 0 1-.6765 8.1042v-5.6772a.79.79 0 0 0-.407-.667zm2.0107-3.0231l-.142-.0852-4.7735-2.7818a.7759.7759 0 0 0-.7854 0L9.409 9.2297V6.8974a.0662.0662 0 0 1 .0284-.0615l4.8303-2.7866a4.4992 4.4992 0 0 1 6.6802 4.66zM8.3065 12.863l-2.02-1.1638a.0804.0804 0 0 1-.038-.0567V6.0742a4.4992 4.4992 0 0 1 7.3757-3.4537l-.142.0805L8.704 5.459a.7948.7948 0 0 0-.3927.6813zm1.0976-2.3654l2.602-1.4998 2.6069 1.4998v2.9994l-2.5974 1.4997-2.6067-1.4997Z", 24.0),
        }
    }
}

/// Cache de texturas (ícone × tamanho em pixels físicos).
#[derive(Default)]
pub struct IconCache {
    textures: HashMap<(Icon, u32), TextureHandle>,
}

impl IconCache {
    fn texture(&mut self, ctx: &egui::Context, icon: Icon, px: u32) -> &TextureHandle {
        self.textures.entry((icon, px)).or_insert_with(|| {
            let (d, vb) = icon.path();
            let polys = flatten_path(d);
            let size = px.max(1) as usize;
            let cov = rasterize(&polys, vb, size, size);
            let pixels: Vec<Color32> = cov
                .iter()
                .map(|&c| {
                    let a = (c.clamp(0.0, 1.0) * 255.0).round() as u8;
                    Color32::from_rgba_premultiplied(a, a, a, a)
                })
                .collect();
            let img = egui::ColorImage::new([size, size], pixels);
            ctx.load_texture(format!("icon-{icon:?}-{px}"), img, TextureOptions::LINEAR)
        })
    }

    /// Pinta o ícone centralizado em `rect` (quadrado de lado `size` pontos), tingido com `color`
    /// e girado `angle` radianos em torno do centro (para os chevrons animados).
    pub fn paint(&mut self, painter: &egui::Painter, icon: Icon, center: Pos2, size: f32, color: Color32, angle: f32) {
        let ppp = painter.pixels_per_point();
        let px = (size * ppp).round().max(1.0) as u32;
        let tex = self.texture(painter.ctx(), icon, px).id();
        let rect = Rect::from_center_size(center, Vec2::splat(px as f32 / ppp));
        let mut mesh = Mesh::with_texture(tex);
        mesh.add_rect_with_uv(rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), color);
        if angle != 0.0 {
            let rot = egui::emath::Rot2::from_angle(angle);
            for v in &mut mesh.vertices {
                v.pos = center + rot * (v.pos - center);
            }
        }
        painter.add(egui::Shape::mesh(mesh));
    }
}

// ---------------------------------------------------------------------------
// Parser de `path d` (M L H V C S Q T A Z, absolutos e relativos) → polígonos.
// ---------------------------------------------------------------------------

type Poly = Vec<(f32, f32)>;

struct Tokens<'a> {
    s: &'a [u8],
    i: usize,
}

impl Tokens<'_> {
    fn skip_sep(&mut self) {
        while self.i < self.s.len() && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',') {
            self.i += 1;
        }
    }

    fn peek_cmd(&mut self) -> Option<u8> {
        self.skip_sep();
        self.s.get(self.i).copied().filter(|c| c.is_ascii_alphabetic() && *c != b'e' && *c != b'E')
    }

    fn at_number(&mut self) -> bool {
        self.skip_sep();
        matches!(self.s.get(self.i), Some(c) if c.is_ascii_digit() || *c == b'-' || *c == b'+' || *c == b'.')
    }

    /// Lê um número no formato compacto do SVG (`.5.5`, `1.06-1.06`, `1e-3`).
    fn number(&mut self) -> Option<f32> {
        self.skip_sep();
        let start = self.i;
        let s = self.s;
        let mut i = self.i;
        if i < s.len() && (s[i] == b'-' || s[i] == b'+') {
            i += 1;
        }
        let mut seen_dot = false;
        let mut digits = 0;
        while i < s.len() {
            match s[i] {
                b'0'..=b'9' => digits += 1,
                b'.' if !seen_dot => seen_dot = true,
                _ => break,
            }
            i += 1;
        }
        if digits == 0 {
            return None;
        }
        if i < s.len() && (s[i] == b'e' || s[i] == b'E') {
            let mut j = i + 1;
            if j < s.len() && (s[j] == b'-' || s[j] == b'+') {
                j += 1;
            }
            if j < s.len() && s[j].is_ascii_digit() {
                while j < s.len() && s[j].is_ascii_digit() {
                    j += 1;
                }
                i = j;
            }
        }
        self.i = i;
        std::str::from_utf8(&s[start..i]).ok()?.parse().ok()
    }

    /// Flags de arco podem vir coladas (`0 01.5 2`): lê um único dígito.
    fn flag(&mut self) -> Option<bool> {
        self.skip_sep();
        let c = *self.s.get(self.i)?;
        self.i += 1;
        match c {
            b'0' => Some(false),
            b'1' => Some(true),
            _ => None,
        }
    }
}

const CURVE_STEPS: usize = 16;

pub(crate) fn flatten_path(d: &str) -> Vec<Poly> {
    let mut t = Tokens { s: d.as_bytes(), i: 0 };
    let mut polys: Vec<Poly> = Vec::new();
    let mut cur: Poly = Vec::new();
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let (mut sx, mut sy) = (0.0f32, 0.0f32);
    // Último ponto de controle (para S/T).
    let mut last_ctrl: Option<(f32, f32)> = None;
    let mut last_cmd = b' ';
    let mut cmd = b' ';

    let close = |cur: &mut Poly, polys: &mut Vec<Poly>| {
        if cur.len() > 2 {
            polys.push(std::mem::take(cur));
        } else {
            cur.clear();
        }
    };

    loop {
        if let Some(c) = t.peek_cmd() {
            cmd = c;
            t.i += 1;
        } else if !t.at_number() {
            break;
        } else if cmd == b'M' {
            cmd = b'L'; // pares extras após M viram L
        } else if cmd == b'm' {
            cmd = b'l';
        }
        let rel = cmd.is_ascii_lowercase();
        let (ox, oy) = if rel { (x, y) } else { (0.0, 0.0) };
        match cmd.to_ascii_uppercase() {
            b'M' => {
                let (Some(nx), Some(ny)) = (t.number(), t.number()) else { break };
                close(&mut cur, &mut polys);
                x = ox + nx;
                y = oy + ny;
                sx = x;
                sy = y;
                cur.push((x, y));
                last_ctrl = None;
            }
            b'L' => {
                let (Some(nx), Some(ny)) = (t.number(), t.number()) else { break };
                x = ox + nx;
                y = oy + ny;
                cur.push((x, y));
                last_ctrl = None;
            }
            b'H' => {
                let Some(nx) = t.number() else { break };
                x = ox + nx;
                cur.push((x, y));
                last_ctrl = None;
            }
            b'V' => {
                let Some(ny) = t.number() else { break };
                y = oy + ny;
                cur.push((x, y));
                last_ctrl = None;
            }
            b'C' | b'S' => {
                let (c1x, c1y) = if cmd.eq_ignore_ascii_case(&b'C') {
                    let (Some(a), Some(b)) = (t.number(), t.number()) else { break };
                    (ox + a, oy + b)
                } else {
                    match (last_ctrl, last_cmd.to_ascii_uppercase()) {
                        (Some((lx, ly)), b'C' | b'S') => (2.0 * x - lx, 2.0 * y - ly),
                        _ => (x, y),
                    }
                };
                let (Some(a), Some(b), Some(ex), Some(ey)) = (t.number(), t.number(), t.number(), t.number()) else { break };
                let (c2x, c2y) = (ox + a, oy + b);
                let (ex, ey) = (ox + ex, oy + ey);
                for i in 1..=CURVE_STEPS {
                    let u = i as f32 / CURVE_STEPS as f32;
                    let m = 1.0 - u;
                    let px = m * m * m * x + 3.0 * m * m * u * c1x + 3.0 * m * u * u * c2x + u * u * u * ex;
                    let py = m * m * m * y + 3.0 * m * m * u * c1y + 3.0 * m * u * u * c2y + u * u * u * ey;
                    cur.push((px, py));
                }
                last_ctrl = Some((c2x, c2y));
                x = ex;
                y = ey;
            }
            b'Q' | b'T' => {
                let (cx, cy) = if cmd.eq_ignore_ascii_case(&b'Q') {
                    let (Some(a), Some(b)) = (t.number(), t.number()) else { break };
                    (ox + a, oy + b)
                } else {
                    match (last_ctrl, last_cmd.to_ascii_uppercase()) {
                        (Some((lx, ly)), b'Q' | b'T') => (2.0 * x - lx, 2.0 * y - ly),
                        _ => (x, y),
                    }
                };
                let (Some(ex), Some(ey)) = (t.number(), t.number()) else { break };
                let (ex, ey) = (ox + ex, oy + ey);
                for i in 1..=CURVE_STEPS {
                    let u = i as f32 / CURVE_STEPS as f32;
                    let m = 1.0 - u;
                    cur.push((m * m * x + 2.0 * m * u * cx + u * u * ex, m * m * y + 2.0 * m * u * cy + u * u * ey));
                }
                last_ctrl = Some((cx, cy));
                x = ex;
                y = ey;
            }
            b'A' => {
                let (Some(rx), Some(ry), Some(rot)) = (t.number(), t.number(), t.number()) else { break };
                let (Some(large), Some(sweep)) = (t.flag(), t.flag()) else { break };
                let (Some(ex), Some(ey)) = (t.number(), t.number()) else { break };
                let (ex, ey) = (ox + ex, oy + ey);
                arc_points(&mut cur, (x, y), (ex, ey), rx, ry, rot, large, sweep);
                x = ex;
                y = ey;
                last_ctrl = None;
            }
            b'Z' => {
                close(&mut cur, &mut polys);
                x = sx;
                y = sy;
                cur.push((x, y));
                last_ctrl = None;
            }
            _ => break,
        }
        last_cmd = cmd;
    }
    close(&mut cur, &mut polys);
    polys
}

/// Converte um arco elíptico (parametrização por extremos, SVG 1.1 F.6.5) em segmentos.
#[allow(clippy::too_many_arguments)]
fn arc_points(out: &mut Poly, p0: (f32, f32), p1: (f32, f32), rx: f32, ry: f32, rot_deg: f32, large: bool, sweep: bool) {
    if p0 == p1 {
        return;
    }
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx == 0.0 || ry == 0.0 {
        out.push(p1);
        return;
    }
    let phi = rot_deg.to_radians();
    let (sin_p, cos_p) = phi.sin_cos();
    let dx = (p0.0 - p1.0) / 2.0;
    let dy = (p0.1 - p1.1) / 2.0;
    let x1 = cos_p * dx + sin_p * dy;
    let y1 = -sin_p * dx + cos_p * dy;
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut coef = if den == 0.0 { 0.0 } else { (num / den).max(0.0).sqrt() };
    if large == sweep {
        coef = -coef;
    }
    let cx1 = coef * rx * y1 / ry;
    let cy1 = -coef * ry * x1 / rx;
    let cx = cos_p * cx1 - sin_p * cy1 + (p0.0 + p1.0) / 2.0;
    let cy = sin_p * cx1 + cos_p * cy1 + (p0.1 + p1.1) / 2.0;
    let angle = |ux: f32, uy: f32, vx: f32, vy: f32| {
        let dot = ux * vx + uy * vy;
        let len = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
        let mut a = (dot / len).clamp(-1.0, 1.0).acos();
        if ux * vy - uy * vx < 0.0 {
            a = -a;
        }
        a
    };
    let ux = (x1 - cx1) / rx;
    let uy = (y1 - cy1) / ry;
    let vx = (-x1 - cx1) / rx;
    let vy = (-y1 - cy1) / ry;
    let theta1 = angle(1.0, 0.0, ux, uy);
    let mut dtheta = angle(ux, uy, vx, vy);
    let tau = std::f32::consts::TAU;
    if !sweep && dtheta > 0.0 {
        dtheta -= tau;
    } else if sweep && dtheta < 0.0 {
        dtheta += tau;
    }
    let steps = ((dtheta.abs() / tau) * 48.0).ceil().max(4.0) as usize;
    for i in 1..=steps {
        let th = theta1 + dtheta * i as f32 / steps as f32;
        let (s, c) = th.sin_cos();
        out.push((cos_p * rx * c - sin_p * ry * s + cx, sin_p * rx * c + cos_p * ry * s + cy));
    }
}

/// Rasteriza os polígonos (coordenadas no viewBox `vb`×`vb`) numa grade `w`×`h` de cobertura 0..1.
pub(crate) fn rasterize(polys: &[Poly], vb: f32, w: usize, h: usize) -> Vec<f32> {
    const SUB: usize = 5; // sub-linhas por pixel
    let sx = w as f32 / vb;
    let sy = h as f32 / vb;
    let mut edges: Vec<(f32, f32, f32, f32)> = Vec::new();
    for p in polys {
        for i in 0..p.len() {
            let a = p[i];
            let b = p[(i + 1) % p.len()];
            if a.1 != b.1 {
                edges.push((a.0 * sx, a.1 * sy, b.0 * sx, b.1 * sy));
            }
        }
    }
    let mut cov = vec![0.0f32; w * h];
    let mut xs: Vec<(f32, i32)> = Vec::new();
    let weight = 1.0 / SUB as f32;
    for py in 0..h {
        let row = &mut cov[py * w..(py + 1) * w];
        for s in 0..SUB {
            let y = py as f32 + (s as f32 + 0.5) / SUB as f32;
            xs.clear();
            for &(x0, y0, x1, y1) in &edges {
                let (lo, hi) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
                if y < lo || y >= hi {
                    continue;
                }
                let t = (y - y0) / (y1 - y0);
                xs.push((x0 + t * (x1 - x0), if y1 > y0 { 1 } else { -1 }));
            }
            xs.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut winding = 0;
            let mut start = 0.0;
            for &(x, dir) in &xs {
                let before = winding;
                winding += dir;
                if before == 0 && winding != 0 {
                    start = x;
                } else if before != 0 && winding == 0 {
                    add_span(row, start, x, weight);
                }
            }
        }
    }
    cov
}

fn add_span(row: &mut [f32], x0: f32, x1: f32, weight: f32) {
    let w = row.len() as f32;
    let (x0, x1) = (x0.clamp(0.0, w), x1.clamp(0.0, w));
    if x1 <= x0 {
        return;
    }
    let i0 = x0.floor() as usize;
    let i1 = x1.floor() as usize;
    if i0 == i1 {
        row[i0.min(row.len() - 1)] += (x1 - x0) * weight;
        return;
    }
    row[i0] += (i0 as f32 + 1.0 - x0) * weight;
    for v in row.iter_mut().take(i1).skip(i0 + 1) {
        *v += weight;
    }
    if i1 < row.len() {
        row[i1] += (x1 - i1 as f32) * weight;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_compact_numbers() {
        let mut t = Tokens { s: b".5.5-1.06-1.06 1e1,2", i: 0 };
        let v: Vec<f32> = std::iter::from_fn(|| t.number()).collect();
        assert_eq!(v, vec![0.5, 0.5, -1.06, -1.06, 10.0, 2.0]);
    }

    #[test]
    fn square_fills_fully() {
        let polys = flatten_path("M0 0H16V16H0z");
        let cov = rasterize(&polys, 16.0, 8, 8);
        assert!(cov.iter().all(|c| (c - 1.0).abs() < 1e-3), "{cov:?}");
    }

    #[test]
    fn hole_with_nonzero_rule() {
        // Quadrado externo horário + interno anti-horário = moldura (furo no meio).
        let polys = flatten_path("M0 0H16V16H0zM4 4V12H12V4z");
        let cov = rasterize(&polys, 16.0, 16, 16);
        assert!(cov[8 * 16 + 8] < 0.01);
        assert!(cov[16 + 1] > 0.99);
    }

    #[test]
    fn every_icon_rasterizes() {
        for icon in [
            Icon::TerminalBox,
            Icon::Close,
            Icon::Mic,
            Icon::Attach,
            Icon::CloudUpload,
            Icon::TreeChevron,
            Icon::FolderOpen,
            Icon::FolderClosed,
            Icon::FileGeneric,
            Icon::Chat,
            Icon::Diagram,
            Icon::GitBranch,
            Icon::GitHub,
            Icon::Refresh,
            Icon::ArrowDown,
            Icon::ArrowUp,
            Icon::Undo,
            Icon::Split,
            Icon::ClaudeLogo,
            Icon::AntigravityLogo,
            Icon::OllamaLogo,
            Icon::CodexLogo,
        ] {
            let (d, vb) = icon.path();
            let polys = flatten_path(d);
            assert!(!polys.is_empty(), "{icon:?}");
            let cov = rasterize(&polys, vb, 24, 24);
            let total: f32 = cov.iter().sum();
            assert!(total > 10.0, "{icon:?} quase vazio: {total}");
            assert!(cov.iter().all(|c| *c <= 1.0001), "{icon:?} cobertura > 1");
        }
    }

    #[test]
    fn agent_logos_rasterize_cleanly() {
        for (icon, name) in
            [(Icon::ClaudeLogo, "Claude"), (Icon::AntigravityLogo, "Antigravity"), (Icon::OllamaLogo, "Ollama"), (Icon::CodexLogo, "Codex")]
        {
            let (d, vb) = icon.path();
            let polys = flatten_path(d);
            assert!(!polys.is_empty(), "{name} sem polígonos");
            let cov = rasterize(&polys, vb, 24, 24);
            let total: f32 = cov.iter().sum();
            assert!(total > 20.0 && total < 400.0, "{name} cobertura estranha: {total}");
        }
    }

    #[test]
    fn the_branch_icon_keeps_its_holes() {
        // Os círculos do Octicons são anéis: o centro (furo) fica vazio com a regra nonzero.
        let (d, vb) = Icon::GitBranch.path();
        let cov = rasterize(&flatten_path(d), vb, 64, 64);
        let at = |x: f32, y: f32| cov[(y / vb * 64.0) as usize * 64 + (x / vb * 64.0) as usize];
        assert!(at(4.25, 3.25) < 0.3, "furo do círculo de cima à esquerda");
        assert!(at(11.75, 3.25) < 0.3, "furo do círculo da direita");
        assert!(at(4.25, 12.75) < 0.3, "furo do círculo de baixo");
        assert!(at(4.25, 8.0) > 0.7, "o traço entre os círculos é cheio");
    }
}
