import { useStore } from "../store";
import type { ActiveTab } from "../store";
import "./TabBar.css";

const AGENT_TABS: { id: ActiveTab; label: string }[] = [
  { id: "chat", label: "Chat" },
  { id: "tasks", label: "Tasks" },
  { id: "workspace", label: "Workspace" },
  { id: "activity", label: "Activity" },
  { id: "profile", label: "Profile" },
];

const CHANNEL_TABS: { id: ActiveTab; label: string }[] = [
  { id: "chat", label: "Chat" },
  { id: "tasks", label: "Tasks" },
];

export function TabBar() {
  const { currentAgent, activeTab, setActiveTab } = useStore();
  const tabs = currentAgent ? AGENT_TABS : CHANNEL_TABS;

  return (
    <div className="tab-bar">
      {tabs.map((tab) => (
        <button
          key={tab.id}
          onClick={() => setActiveTab(tab.id)}
          className={`tab-bar__item${activeTab === tab.id ? " is-active" : ""}`}
        >
          {tab.label}
        </button>
      ))}
    </div>
  );
}
