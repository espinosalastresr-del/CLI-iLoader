# iloader CLI

Herramienta de línea de comandos para instalar IPAs en iPhone/iPad desde **Alpine Linux** (y cualquier Linux con musl libc).

Extrae toda la lógica de autenticación y sideloading del backend de iloader (basado en `isideload` + `idevice`) sin depender de Tauri ni de herramientas glibc como AltServer o PlumeImpactor.

---

## Instalación

### Opción 1: Descargar binario precompilado (recomendado)

```sh
# Alpine x86_64
wget https://github.com/TU_USUARIO/iloader-cli/releases/latest/download/iloader-linux-x86_64
chmod +x iloader-linux-x86_64
mv iloader-linux-x86_64 /usr/local/bin/iloader
```

### Opción 2: Compilar desde fuente

```sh
# Requiere Rust + cross-rs
cargo install cross
cross build --release --target x86_64-unknown-linux-musl
# El binario queda en: target/x86_64-unknown-linux-musl/release/iloader
```

---

## Requisitos en Alpine

```sh
# Dependencias del sistema
apk add usbmuxd libimobiledevice docker

# Iniciar servicios
rc-update add usbmuxd && service usbmuxd start
rc-update add docker  && service docker  start

# Levantar servidor anisette (necesario para autenticarse con Apple)
docker run -d --restart always \
  -p 6969:6969 \
  --volume anisette_data:/home/Alcoholic/.config/anisette-v3/lib/ \
  --name anisette \
  dadoum/anisette-v3-server:latest
```

---

## Uso

### Instalar SideStore (todo en uno)

```sh
iloader install-sidestore -a tuapple@id.com -p tupassword
```

Esto descarga SideStore, lo firma, lo instala y coloca el pairing file automáticamente.

### Instalar cualquier IPA

```sh
# Desde archivo local
iloader install -a tuapple@id.com -p tupassword ./MiApp.ipa

# Desde URL (descarga automática)
iloader install -a tuapple@id.com -p tupassword \
  https://github.com/SideStore/SideStore/releases/latest/download/SideStore.ipa
```

### Listar dispositivos conectados

```sh
iloader devices
```

### Gestionar el pairing file

```sh
# Colocar en SideStore
iloader pairing --app sidestore

# Exportar a disco (para uso manual)
iloader pairing --export ~/pairing.plist

# Apps soportadas: sidestore, feather, livecont, stikdebug, sparebox
```

### Gestionar certificados de desarrollo

```sh
# Ver certificados activos
iloader certs -a tuapple@id.com -p tupassword

# Revocar un certificado por serial
iloader revoke-cert -a tuapple@id.com -p tupassword SERIAL123

# Ver App IDs usados
iloader app-ids -a tuapple@id.com -p tupassword
```

---

## Variables de entorno

Para no escribir credenciales en la línea de comandos:

```sh
export ILOADER_APPLE_ID="tuapple@id.com"
export ILOADER_APPLE_PASSWORD="tupassword"
export ILOADER_ANISETTE="http://localhost:6969"  # por defecto

iloader install-sidestore
```

---

## Compilar desde GitHub Actions (sin máquina potente)

1. Haz fork o crea un repositorio con este código en GitHub
2. Ve a **Actions** → **Build & Release (Alpine/musl)** → **Run workflow**
3. Cuando termine, descarga el artefacto desde la pestaña de Actions
4. O crea un tag: `git tag v0.1.0 && git push --tags` para generar un Release automático

El binario resultante es **completamente estático** (musl), no requiere ninguna librería del sistema.
