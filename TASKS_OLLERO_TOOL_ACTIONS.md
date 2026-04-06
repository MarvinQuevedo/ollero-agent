# Ollero: tareas minimas por iteracion

## Objetivo operativo
Trabajar en cambios pequenos, verificables y acumulables para mejorar `ollero` sin sobrecargar el script autonomo.

## Modelo recomendado para tool actions
- **Modelo principal**: `qwen3.5:9b`
- **Razon**: buen equilibrio calidad/latencia para tu hardware local (i7-12700F, 64 GB RAM, RTX 4070 SUPER), con soporte moderno de tool calling en Ollama.
- **Comando**: `ollama pull qwen3.5:9b`

## Reglas de ejecucion (siempre)
1. Ejecutar solo **1 tarea por corrida**.
2. Limitar cambios a los archivos listados en la tarea.
3. No mezclar refactor grande con feature nueva.
4. Antes de editar, leer archivos objetivo completos.
5. Al final de cada tarea: compilar, correr pruebas puntuales y registrar resultado.
6. Si falla una prueba, crear subtarea de correccion en lugar de expandir alcance.
7. Mantener respuestas del modelo concretas y orientadas a diff.
8. No crear commit hasta validar manualmente la tarea.

## Modo autonomo del CLI (nuevo)
- `--autonomous`: activa loop multi-ronda con tools para `bash`, `web_search` y `web_fetch`.
- `--max-rounds <n>`: limita rondas de tool use (evita ciclos largos).
- `--cmd-timeout-ms <n>`: timeout de comandos shell.
- `--allow-bash` y `--allow-web`: permisos granulares si no quieres autonomia total.
- `--verbose`: imprime logs paso a paso (rondas, tool calls, inicio/fin, guardado y pruning de corridas).

### Ejemplos
- `node --experimental-strip-types scripts/ollero-cli.ts ask "diagnostica este repo" --autonomous`
- `node --experimental-strip-types scripts/ollero-cli.ts run T03 --autonomous`
- `node --experimental-strip-types scripts/ollero-cli.ts ask "busca docs de ollama tools" --allow-web`

## Cola automatizada (T01–T10 en orden)

Ejecuta las tareas **una por una** con validacion (`cargo test` / patrones por tarea) y parada tras **3 fallos consecutivos**:

```text
npx tsx scripts/run-task-queue.ts
```

Por defecto **no** exige escritura de archivos (evita bucles read-only del modelo). Para exigir al menos un `write`/`replace` en tareas de documentacion, usa **`--strict`** o `OLLERO_QUEUE_STRICT=1`.

Solo listar sin ejecutar: `--dry-list`. Desde una tarea: `--from T04`. Hasta una tarea: `--to T06`.

Variables opcionales: `OLLERO_QUEUE_MODEL` (default `qwen3-coder:30b`), `OLLERO_QUEUE_MAX_ROUNDS`, `OLLERO_QUEUE_MAX_ERRORS`.

## Smoke manual (checklist minimo, T06)

- `npx tsx scripts/ollero-cli.ts list` — lista IDs de tareas.
- `npx tsx scripts/ollero-cli.ts show T01` — muestra prompt de T01.
- `npx tsx scripts/ollero-cli.ts run T01 --dry-run` — no llama a Ollama.
- Ollama detenido: una corrida debe fallar con error claro de conexion.
- Modelo inexistente (`--model no-existe-xyz`): error del servidor o del cliente manejable.

## TASK T01 - Verificar baseline local
### Subtareas
- Confirmar que Ollama responde y el modelo existe.
- Confirmar que `ollero` compila sin cambios.
- Capturar baseline de pruebas actuales.

### Archivos involucrados
- `Cargo.toml`
- `src/main.rs`

### PROMPT
```text
Actua como ingeniero de release local.

Objetivo:
1) verificar baseline del proyecto
2) no hacer cambios de codigo todavia

Pasos:
- Ejecuta comandos minimos para validar que Ollama esta activo y que qwen3.5:9b esta instalado.
- Compila el proyecto Rust y ejecuta pruebas existentes.
- Entrega un reporte corto con: estado de compilacion, pruebas, bloqueadores.

Restricciones:
- Sin editar archivos.
- Sin crear commit.
```

## TASK T02 - Documentar modo CLI de pruebas
### Subtareas
- Crear guia rapida para ejecutar prompts por tarea.
- Incluir ejemplos de uso basico y depuracion.

### Archivos involucrados
- `TASKS_OLLERO_TOOL_ACTIONS.md`
- `scripts/ollero-cli.ts`

### PROMPT
```text
Actua como technical writer.

Objetivo:
Documentar como ejecutar el modo CLI simple para pruebas del agente por partes.

Entrega:
- seccion con comandos de uso real
- explicacion de list/show/run/ask
- flujo recomendado para depuracion incremental

Restricciones:
- No cambiar logica de Rust.
- Mantener documentacion corta y accionable.
```

## TASK T03 - Ejecutar una tarea por ID
### Subtareas
- Implementar carga de tareas desde markdown.
- Permitir seleccionar tarea por ID y enviar su prompt.

### Archivos involucrados
- `scripts/ollero-cli.ts`
- `TASKS_OLLERO_TOOL_ACTIONS.md`

### PROMPT
```text
Actua como desarrollador TypeScript.

Objetivo:
Implementar o mejorar un CLI que:
- liste tareas
- muestre detalle de tarea
- ejecute el prompt de una tarea por su ID contra /api/chat de Ollama

Criterios:
- sin dependencias externas obligatorias
- manejo de errores claro
- salida util para debugging
```

## TASK T04 - Guardar trazas de ejecucion
### Subtareas
- Persistir prompt + respuesta + metricas por corrida.
- Crear carpeta de salidas reutilizable.

### Archivos involucrados
- `scripts/ollero-cli.ts`

### PROMPT
```text
Actua como ingeniero de observabilidad.

Objetivo:
Asegurar que cada ejecucion del CLI guarde:
- prompt enviado
- respuesta del modelo
- metrica de tokens y tool calls (si viene en respuesta)

Formato:
- archivo markdown por corrida
- nombre con timestamp
```

## TASK T05 - Validar modo ask directo
### Subtareas
- Permitir prompt libre para pruebas rapidas.
- Mantener mismo pipeline de guardado de resultados.

### Archivos involucrados
- `scripts/ollero-cli.ts`

### PROMPT
```text
Actua como QA engineer.

Objetivo:
Agregar o validar comando "ask" para enviar un prompt directo al modelo
sin depender de IDs de tareas.

Criterios:
- UX simple desde terminal
- salida legible
- guardado de evidencia de la corrida
```

## TASK T06 - Definir pruebas manuales cortas
### Subtareas
- Crear checklist de smoke tests del CLI.
- Cubrir casos felices y errores comunes.

### Archivos involucrados
- `TASKS_OLLERO_TOOL_ACTIONS.md`

### PROMPT
```text
Actua como test lead.

Objetivo:
Proponer un checklist de pruebas manuales minimas para el CLI:
- list/show/run/ask
- task id inexistente
- Ollama no disponible
- modelo invalido

Salida:
- lista de pasos + resultado esperado por paso
```

## TASK T07 - Integrar workflow con Ollero Rust
### Subtareas
- Definir como usar CLI TS junto con REPL Rust sin conflictos.
- Proponer secuencia operativa diaria.

### Archivos involucrados
- `src/repl/mod.rs`
- `scripts/ollero-cli.ts`
- `TASKS_OLLERO_TOOL_ACTIONS.md`

### PROMPT
```text
Actua como architect de herramientas locales.

Objetivo:
Definir un workflow simple donde:
- REPL Rust de ollero se usa para trabajo interactivo
- CLI TS se usa para pruebas controladas por tarea y debugging

Entrega:
- flujo recomendado paso a paso
- criterios para elegir cada modo
```

## TASK T08 - Endurecer reglas de alcance
### Subtareas
- Evitar cambios fuera de archivos permitidos por tarea.
- Reforzar criterio de "una tarea, un objetivo".

### Archivos involucrados
- `TASKS_OLLERO_TOOL_ACTIONS.md`

### PROMPT
```text
Actua como guardian de alcance.

Objetivo:
Mejorar reglas para evitar sobrecarga del agente:
- cambios pequenos
- validacion por iteracion
- rollback facil

Salida:
- reglas concretas en lenguaje imperativo
- ejemplos de "si/no hacer"
```

## TASK T09 - Preparar lote para commit
### Subtareas
- Resumir cambios aplicados y evidencia de prueba.
- Redactar mensaje de commit orientado a intencion.

### Archivos involucrados
- `scripts/ollero-cli.ts`
- `TASKS_OLLERO_TOOL_ACTIONS.md`

### PROMPT
```text
Actua como maintainer del repositorio.

Objetivo:
Preparar el lote de cambios para commit:
- resumen tecnico breve
- validaciones ejecutadas
- riesgos abiertos
- propuesta de commit message

Restricciones:
- no ejecutar push
- no hacer commit automatico
```

## TASK T10 - Plan de continuacion
### Subtareas
- Definir siguiente bloque de mejoras.
- Priorizar por impacto y riesgo.

### Archivos involucrados
- `TASKS_OLLERO_TOOL_ACTIONS.md`

### PROMPT
```text
Actua como planner tecnico.

Objetivo:
Definir siguientes 5 mejoras despues de estabilizar el CLI y el flujo por tareas.

Formato:
- prioridad alta/media/baja
- esfuerzo estimado (S/M/L)
- riesgo principal
```
