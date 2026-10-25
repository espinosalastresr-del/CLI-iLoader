# Análisis de IPA con REA

Este flujo está aislado en la rama `feature/rea-ipa-analysis` y no cambia el flujo de compilación/instalación de iloader.

## Preparación

1. Abre **Settings → Secrets and variables → Actions** del repositorio.
2. Crea el secreto `IPA_URL` con una URL HTTPS de descarga del IPA que tienes autorización para analizar.
3. Abre **Actions → REA IPA Analysis → Run workflow**.
4. Al terminar, descarga el artefacto `rea-ipa-analysis` de la ejecución.

No pongas contraseñas ni tokens en el YAML ni en la URL si puedes evitarlo. El secreto de URL no se imprime en los logs. Los informes pueden contener nombres de clases, rutas, cadenas, URLs y otros datos internos del IPA; revisa el acceso al artefacto antes de compartirlo.

## Qué hace el workflow

- Descarga el IPA con reintentos, calcula SHA-256 y verifica la estructura ZIP.
- Instala una versión fijada de `rea-agents` en el runner temporal.
- Ejecuta `rea inspect-artifact` sobre el IPA y conserva JSON, stderr y código de salida.
- Extrae el archivo con comprobaciones contra rutas ZIP inseguras y un límite de 5 GiB de contenido expandido.
- Genera inventario de archivos, hashes, frameworks, extensiones, plists y tipos de archivo.
- Si encuentra `global-metadata.dat`, ejecuta `rea inspect-il2cpp-metadata` para cada archivo.
- Publica los informes durante siete días, incluso cuando una fase de análisis falla.

El IPA no se adjunta al artefacto de informes. El workflow no instala ni ejecuta la aplicación.

## Límites

Este es un pipeline de análisis estático y extracción de evidencia. No significa que un agente LLM haya reconstruido automáticamente todas las funciones nativas. El análisis de funciones nativas puede necesitar una operación REA compatible, herramientas adicionales y un proveedor de modelo configurado. No se ha añadido ninguna clave de proveedor ni se ha enviado el IPA a un servicio externo de IA.

Si el archivo no contiene `global-metadata.dat`, el workflow lo informa; eso no demuestra por sí solo que no sea una aplicación Unity. Puede usar Mono, una estructura distinta, protección/cifrado u otro motor. FairPlay puede impedir inspeccionar el ejecutable como código nativo normal.

## Archivos principales

- `rea-ipa-inspection.json`: salida estructurada de REA para el IPA.
- `ipa-files.json`: inventario con rutas, tamaños, cifrado ZIP y SHA-256 por archivo extraído.
- `unity-candidates.json`: archivos potencialmente relacionados con Unity/IL2CPP.
- `frameworks-extensions.txt`: frameworks, extensiones y bundles detectados por ruta.
- `native-binaries.txt`: archivos que la utilidad `file` identifica como Mach-O o bibliotecas nativas.
- `il2cpp/`: resultados y errores por cada metadato IL2CPP encontrado.
- `ipa-sha256.txt`, `ipa-integrity.txt`: integridad del archivo original.

Un código de salida cero no garantiza cobertura completa. Conserva los mensajes de error, limitaciones y evidencia parcial junto con los resultados.
