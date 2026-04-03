import { useStore } from "../store";
import { useInbox } from "../hooks/data";
import type { ActiveTab } from "../store";
import "./TabBar.css";

const AGENT_TABS: { id: ActiveTab; label: string }[] = [
  { id: "chat", label: "Chat" },
  { id: "tasks", label: "Tasks" },
  { id: "workspace", label: "Workspace" },
  { id: "activity", label: "Activity" },
  { id: "profile", label: "Profile" },
];

export function TabBar() {
  const { currentChannel, currentAgent, activeTab, setActiveTab } = useStore();
  const { getConversationThreadUnread } = useInbox();
  const threadUnread = getConversationThreadUnread(currentChannel?.id);
  const channelTabs: { id: ActiveTab; label: string }[] = [
    { id: "chat", label: "Chat" },
    {
      id: "threads",
      label: threadUnread > 0 ? `Threads (${threadUnread})` : "Threads",
    },
    { id: "tasks", label: "Tasks" },
  ];
  const tabs = currentAgent ? AGENT_TABS : channelTabs;

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
