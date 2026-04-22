import { useEffect } from "react";
import { LoaderCircle } from "lucide-react";
import type { AgentEnvVar, RuntimeCatalogEntry } from "./types";
import { useRuntimeModels } from "../../hooks/useRuntimeModels";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Label } from "@/components/ui/label";
import { FormField } from "@/components/ui/form";
import { Button } from "@/components/ui/button";

export const REASONING_EFFORTS = [
  { value: "default", label: "Default" },
  { value: "none", label: "None" },
  { value: "minimal", label: "Minimal" },
  { value: "low", label: "Low" },
  { value: "medium", label: "Medium" },
  { value: "high", label: "High" },
  { value: "xhigh", label: "Extra High" },
  { value: "max", label: "Max" },
];

const DEFAULT_REASONING_OPTION = REASONING_EFFORTS[0];

export const RUNTIME_OPTIONS = [
  { value: "claude", label: "Claude Code" },
  { value: "codex", label: "Codex CLI" },
  { value: "kimi", label: "Kimi CLI" },
  { value: "opencode", label: "OpenCode" },
  { value: "gemini", label: "Gemini CLI" },
] as const;

export interface AgentConfigState {
  name: string;
  display_name: string;
  description: string;
  systemPrompt: string | null;
  runtime: string;
  model: string;
  reasoningEffort: string | null;
  envVars: AgentEnvVar[];
}

interface Props {
  state: AgentConfigState;
  runtimeStatuses?: RuntimeCatalogEntry[];
  runtimeStatusError?: string | null;
  onChange: (next: AgentConfigState) => void;
}

function findRuntimeStatus(
  runtime: string,
  runtimeStatuses: RuntimeCatalogEntry[] = [],
): RuntimeCatalogEntry | undefined {
  return runtimeStatuses.find((entry) => entry.runtime === runtime);
}

function fallbackRuntimeLabel(runtime: string): string {
  return (
    RUNTIME_OPTIONS.find((option) => option.value === runtime)?.label ?? runtime
  );
}

function reasoningEffortLabel(value: string): string {
  return (
    REASONING_EFFORTS.find((option) => option.value === value)?.label ?? value
  );
}

export function runtimeCatalog(
  runtimeStatuses: RuntimeCatalogEntry[] = [],
): Array<{ value: string; label: string }> {
  if (runtimeStatuses.length === 0) {
    return RUNTIME_OPTIONS.map((runtime) => ({ ...runtime }));
  }

  return [...runtimeStatuses]
    .sort(
      (left, right) =>
        (left.order ?? Number.MAX_SAFE_INTEGER) -
        (right.order ?? Number.MAX_SAFE_INTEGER),
    )
    .map((runtimeStatus) => ({
      value: runtimeStatus.runtime,
      label: runtimeStatus.label ?? fallbackRuntimeLabel(runtimeStatus.runtime),
    }));
}

export function runtimeReasoningEffortValues(
  runtime: string,
  runtimeStatuses: RuntimeCatalogEntry[] = [],
): string[] {
  return findRuntimeStatus(runtime, runtimeStatuses)?.reasoning_efforts ?? [];
}

export function runtimeSupportsReasoningEffort(
  runtime: string,
  runtimeStatuses: RuntimeCatalogEntry[] = [],
): boolean {
  return runtimeReasoningEffortValues(runtime, runtimeStatuses).length > 0;
}

export function runtimeReasoningEffortOptions(
  runtime: string,
  runtimeStatuses: RuntimeCatalogEntry[] = [],
): Array<{ value: string; label: string }> {
  const efforts = runtimeReasoningEffortValues(runtime, runtimeStatuses);
  if (efforts.length === 0) {
    return [];
  }

  return [
    DEFAULT_REASONING_OPTION,
    ...efforts.map((value) => ({ value, label: reasoningEffortLabel(value) })),
  ];
}

export function normalizeRuntimeReasoningEffort(
  runtime: string,
  reasoningEffort: string | null,
  runtimeStatuses: RuntimeCatalogEntry[] = [],
): string | null {
  const efforts = runtimeReasoningEffortValues(runtime, runtimeStatuses);
  if (efforts.length === 0) {
    return null;
  }

  return reasoningEffort && efforts.includes(reasoningEffort)
    ? reasoningEffort
    : null;
}

export function runtimeOptionLabel(
  runtime: string,
  runtimeStatuses: RuntimeCatalogEntry[] = [],
): string {
  const status = findRuntimeStatus(runtime, runtimeStatuses);
  const baseLabel = status?.label ?? fallbackRuntimeLabel(runtime);
  if (!status) return `${baseLabel} · status unavailable`;
  if (status.auth === "not_installed") return `${baseLabel} · not installed`;
  if (status.auth === "authed") return `${baseLabel} · signed in`;
  return `${baseLabel} · not signed in`;
}

export function isRuntimeAvailable(
  runtime: string,
  runtimeStatuses: RuntimeCatalogEntry[] = [],
): boolean {
  const status = findRuntimeStatus(runtime, runtimeStatuses);
  return status?.auth !== "not_installed" && status?.auth !== undefined;
}

export function runtimeStatusSummary(
  runtime: string,
  runtimeStatuses: RuntimeCatalogEntry[] = [],
): { tone: "ok" | "warn" | "muted"; title: string; detail: string } {
  const status = findRuntimeStatus(runtime, runtimeStatuses);
  if (!status) {
    return {
      tone: "muted",
      title: "Status unavailable",
      detail: "The local runtime probe did not return a status for this CLI.",
    };
  }
  if (status.auth === "not_installed") {
    return {
      tone: "warn",
      title: "Not installed",
      detail: "This runtime is not available on the local machine yet.",
    };
  }
  if (status.auth === "authed") {
    return {
      tone: "ok",
      title: "Signed in",
      detail: "This runtime is installed locally and has an active login.",
    };
  }
  return {
    tone: "warn",
    title: "Not signed in",
    detail:
      "The CLI is installed, but local authentication needs to be completed before agent startup will work reliably.",
  };
}

function groupModelsByProvider(
  models: string[],
): { provider: string; models: string[] }[] {
  const groups = new Map<string, string[]>();
  for (const model of models) {
    const provider = model.includes("/") ? model.split("/")[0] : "(other)";
    const existing = groups.get(provider) ?? [];
    existing.push(model);
    groups.set(provider, existing);
  }
  return Array.from(groups.entries())
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([provider, models]) => ({ provider, models }));
}

export function modelSelectDisplayLabel({
  selectedModel,
  runtimeModels,
  isLoading,
}: {
  selectedModel: string;
  runtimeModels: string[];
  isLoading: boolean;
}): string {
  if (isLoading) return "Loading models…";
  if (selectedModel) return selectedModel;
  if (runtimeModels.length > 0) return runtimeModels[0];
  return "No models available";
}

export function AgentConfigForm({
  state,
  runtimeStatuses = [],
  runtimeStatusError = null,
  onChange,
}: Props) {
  const { runtimeModels, runtimeModelsError, isLoading } = useRuntimeModels(
    state.runtime,
  );

  useEffect(() => {
    if (runtimeModels.length === 0 || runtimeModels.includes(state.model)) {
      return;
    }

    onChange({
      ...state,
      model: runtimeModels[0],
    });
  }, [onChange, runtimeModels, state]);

  function updateEnvVar(index: number, key: keyof AgentEnvVar, value: string) {
    const envVars = state.envVars.map((envVar, envIndex) =>
      envIndex === index ? { ...envVar, [key]: value } : envVar,
    );
    onChange({ ...state, envVars });
  }

  function addEnvVar() {
    onChange({
      ...state,
      envVars: [...state.envVars, { key: "", value: "" }],
    });
  }

  function removeEnvVar(index: number) {
    onChange({
      ...state,
      envVars: state.envVars.filter((_, envIndex) => envIndex !== index),
    });
  }

  const runtimeSummary = runtimeStatusSummary(state.runtime, runtimeStatuses);
  const runtimeOptions = runtimeCatalog(runtimeStatuses);
  const reasoningOptions = runtimeReasoningEffortOptions(
    state.runtime,
    runtimeStatuses,
  );
  const modelLabel = modelSelectDisplayLabel({
    selectedModel: state.model,
    runtimeModels,
    isLoading,
  });

  return (
    <div className="agent-config-form">
      <section className="agent-config-section">
        <div className="agent-config-section-header">
          <span className="agent-config-section-kicker">
            [identity::surface]
          </span>
        </div>
        <div className="agent-config-grid">
          <FormField>
            <Label>Name</Label>
            <Input
              value={state.display_name}
              onChange={(e) =>
                onChange({ ...state, display_name: e.target.value })
              }
              placeholder="e.g. Code Reviewer"
              autoFocus
            />
          </FormField>
        </div>

        <FormField>
          <Label>Role</Label>
          <Textarea
            value={state.description}
            onChange={(e) =>
              onChange({ ...state, description: e.target.value })
            }
            placeholder="What does this agent do?"
          />
          <p className="text-xs text-muted-foreground leading-relaxed mt-1">
            Keep it brief and operational. This description guides how teammates
            interpret the agent.
          </p>
        </FormField>
      </section>

      <section className="agent-config-section">
        <div className="agent-config-section-header">
          <span className="agent-config-section-kicker">
            [runtime::selection]
          </span>
        </div>
        <div className="agent-config-grid">
          <FormField>
            <Label>Runtime</Label>
            <Select
              value={state.runtime}
              onValueChange={(runtime) => {
                onChange({
                  ...state,
                  runtime,
                  model: "",
                  reasoningEffort: normalizeRuntimeReasoningEffort(
                    runtime,
                    state.reasoningEffort,
                    runtimeStatuses,
                  ),
                });
              }}
            >
              <SelectTrigger aria-label="Runtime">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {runtimeOptions.map((runtime) => (
                  <SelectItem
                    key={runtime.value}
                    value={runtime.value}
                    disabled={
                      !isRuntimeAvailable(runtime.value, runtimeStatuses)
                    }
                  >
                    {runtimeOptionLabel(runtime.value, runtimeStatuses)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <div
              className={`runtime-status-banner runtime-status-banner-${runtimeSummary.tone}`}
            >
              <strong>{runtimeSummary.title}</strong>
              <span>{runtimeSummary.detail}</span>
            </div>
            {runtimeStatusError && (
              <p className="text-xs text-muted-foreground leading-relaxed mt-1">
                {runtimeStatusError}
              </p>
            )}
          </FormField>

          <FormField>
            <Label>Model</Label>
            <Select
              value={state.model}
              onValueChange={(model) => onChange({ ...state, model })}
              disabled={isLoading || runtimeModels.length === 0}
            >
              <SelectTrigger aria-label="Model">
                {isLoading ? (
                  <span className="select-trigger-loading">
                    <LoaderCircle
                      size={14}
                      className="select-trigger-spinner"
                    />
                    <span>{modelLabel}</span>
                  </span>
                ) : (
                  <span className="select-trigger-text">{modelLabel}</span>
                )}
              </SelectTrigger>
              <SelectContent className="max-h-[320px] overflow-y-auto">
                {groupModelsByProvider(runtimeModels).map(
                  ({ provider, models }) => (
                    <SelectGroup key={provider}>
                      <SelectLabel>{provider}</SelectLabel>
                      {models.map((model) => (
                        <SelectItem key={model} value={model}>
                          {model.split("/")[1] ?? model}
                        </SelectItem>
                      ))}
                    </SelectGroup>
                  ),
                )}
              </SelectContent>
            </Select>
            {runtimeModelsError && (
              <p className="text-xs text-muted-foreground leading-relaxed mt-1">
                {runtimeModelsError}
              </p>
            )}
          </FormField>

          {reasoningOptions.length > 0 && (
            <FormField>
              <Label>Reasoning</Label>
              <Select
                value={state.reasoningEffort ?? "default"}
                onValueChange={(reasoningEffort) =>
                  onChange({
                    ...state,
                    reasoningEffort,
                  })
                }
              >
                <SelectTrigger aria-label="Reasoning">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {reasoningOptions.map((effort) => (
                    <SelectItem key={effort.value} value={effort.value}>
                      {effort.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </FormField>
          )}
        </div>
      </section>

      <section className="agent-config-section">
        <div className="agent-config-section-header">
          <span className="agent-config-section-kicker">[env::bindings]</span>
          <Button size="sm" variant="ghost" type="button" onClick={addEnvVar}>
            + Add variable
          </Button>
        </div>
        <FormField>
          <Label>Environment Variables</Label>
          <p className="text-xs text-muted-foreground leading-relaxed mt-1">
            Pass runtime secrets and flags into the agent process without
            hardcoding them into prompts.
          </p>
          <div className="env-var-editor">
            {state.envVars.length === 0 && (
              <div className="env-var-editor-empty">
                No environment variables configured.
              </div>
            )}
            {state.envVars.map((envVar, index) => (
              <div key={index} className="env-var-editor-row">
                <Input
                  value={envVar.key}
                  onChange={(e) => updateEnvVar(index, "key", e.target.value)}
                  placeholder="KEY"
                />
                <Input
                  value={envVar.value}
                  onChange={(e) => updateEnvVar(index, "value", e.target.value)}
                  placeholder="value"
                />
                <Button
                  size="sm"
                  variant="ghost"
                  type="button"
                  onClick={() => removeEnvVar(index)}
                >
                  ×
                </Button>
              </div>
            ))}
          </div>
        </FormField>
      </section>
    </div>
  );
}
