Implementada extensão Manifest V3 em [extension/](file:///C:/Users/alice/MyDrive1/Documents/Craft%20Anything/demo1/extension/) com botão flutuante, menu de qualidades reais, rastreamento de rolagem/tela cheia e troca SPA.

Criado host nativo Rust em [demo1_native_host.rs](file:///C:/Users/alice/MyDrive1/Documents/Craft%20Anything/demo1/app/src/bin/demo1_native_host.rs) e servidor IPC em [ipc.rs](file:///C:/Users/alice/MyDrive1/Documents/Craft%20Anything/demo1/app/src/ipc.rs) com auto-inicialização do app e multiplexação de áudio/vídeo via yt-dlp e ffmpeg em [engine.rs](file:///C:/Users/alice/MyDrive1/Documents/Craft%20Anything/demo1/app/src/engine.rs).

Fluxo validado no Microsoft Edge com player HTML5, YouTube real (2160p a áudio) e streams confirmados por ffprobe; capturas salvas em [browser-integration/](file:///C:/Users/alice/MyDrive1/Documents/Craft%20Anything/demo1/checks/codex-review/browser-integration/).
