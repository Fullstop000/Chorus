use anyhow::{anyhow, Result};
use rusqlite::params;
use uuid::Uuid;

use crate::models::*;
use super::Store;

impl Store {
    pub fn create_tasks(
        &self,
        channel_name: &str,
        creator_name: &str,
        titles: &[&str],
    ) -> Result<Vec<TaskInfo>> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::find_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        let max_num: i64 = conn.query_row(
            "SELECT COALESCE(MAX(task_number), 0) FROM tasks WHERE channel_id = ?1",
            params![channel.id],
            |row| row.get(0),
        )?;

        let mut result = Vec::new();
        for (i, title) in titles.iter().enumerate() {
            let id = Uuid::new_v4().to_string();
            let task_number = max_num + 1 + i as i64;
            conn.execute(
                "INSERT INTO tasks (id, channel_id, task_number, title, created_by) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, channel.id, task_number, title, creator_name],
            )?;
            result.push(TaskInfo {
                task_number,
                title: title.to_string(),
                status: "todo".to_string(),
                claimed_by_name: None,
                created_by_name: Some(creator_name.to_string()),
            });
        }
        Ok(result)
    }

    pub fn list_tasks(
        &self,
        channel_name: &str,
        status_filter: Option<TaskStatus>,
    ) -> Result<Vec<TaskInfo>> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::find_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        let map_row = |row: &rusqlite::Row| -> rusqlite::Result<TaskInfo> {
            Ok(TaskInfo {
                task_number: row.get(0)?,
                title: row.get(1)?,
                status: row.get(2)?,
                claimed_by_name: row.get(3)?,
                created_by_name: row.get(4)?,
            })
        };

        let rows: Vec<TaskInfo> = if let Some(status) = status_filter {
            conn.prepare(
                "SELECT task_number, title, status, claimed_by, created_by FROM tasks WHERE channel_id = ?1 AND status = ?2 ORDER BY task_number",
            )?
            .query_map(params![channel.id, status.as_str()], map_row)?
            .filter_map(|r| r.ok())
            .collect()
        } else {
            conn.prepare(
                "SELECT task_number, title, status, claimed_by, created_by FROM tasks WHERE channel_id = ?1 ORDER BY task_number",
            )?
            .query_map(params![channel.id], map_row)?
            .filter_map(|r| r.ok())
            .collect()
        };
        Ok(rows)
    }

    pub fn claim_tasks(
        &self,
        channel_name: &str,
        claimer_name: &str,
        task_numbers: &[i64],
    ) -> Result<Vec<ClaimResult>> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::find_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        let mut results = Vec::new();
        for &tn in task_numbers {
            let task: Option<(String, Option<String>)> = conn
                .query_row(
                    "SELECT status, claimed_by FROM tasks WHERE channel_id = ?1 AND task_number = ?2",
                    params![channel.id, tn],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .ok();

            match task {
                Some((status, claimed_by)) if status == "todo" && claimed_by.is_none() => {
                    conn.execute(
                        "UPDATE tasks SET claimed_by = ?1, status = 'in_progress', updated_at = datetime('now') WHERE channel_id = ?2 AND task_number = ?3",
                        params![claimer_name, channel.id, tn],
                    )?;
                    results.push(ClaimResult { task_number: tn, success: true, reason: None });
                }
                Some(_) => {
                    results.push(ClaimResult {
                        task_number: tn,
                        success: false,
                        reason: Some("task already claimed or not in todo status".to_string()),
                    });
                }
                None => {
                    results.push(ClaimResult {
                        task_number: tn,
                        success: false,
                        reason: Some("task not found".to_string()),
                    });
                }
            }
        }
        Ok(results)
    }

    pub fn unclaim_task(
        &self,
        channel_name: &str,
        claimer_name: &str,
        task_number: i64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::find_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        let claimed_by: Option<String> = conn.query_row(
            "SELECT claimed_by FROM tasks WHERE channel_id = ?1 AND task_number = ?2",
            params![channel.id, task_number],
            |row| row.get(0),
        )?;

        if claimed_by.as_deref() != Some(claimer_name) {
            return Err(anyhow!("task not claimed by {}", claimer_name));
        }

        conn.execute(
            "UPDATE tasks SET claimed_by = NULL, status = 'todo', updated_at = datetime('now') WHERE channel_id = ?1 AND task_number = ?2",
            params![channel.id, task_number],
        )?;
        Ok(())
    }

    pub fn update_task_status(
        &self,
        channel_name: &str,
        task_number: i64,
        requester_name: &str,
        new_status: TaskStatus,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::find_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        let (current_status_str, claimed_by): (String, Option<String>) = conn.query_row(
            "SELECT status, claimed_by FROM tasks WHERE channel_id = ?1 AND task_number = ?2",
            params![channel.id, task_number],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;

        let current_status = TaskStatus::from_str(&current_status_str)
            .ok_or_else(|| anyhow!("invalid task status: {}", current_status_str))?;

        if claimed_by.as_deref() != Some(requester_name) {
            return Err(anyhow!("task not claimed by {}", requester_name));
        }
        if !current_status.can_transition_to(new_status) {
            return Err(anyhow!(
                "cannot transition from {} to {}",
                current_status.as_str(),
                new_status.as_str()
            ));
        }

        conn.execute(
            "UPDATE tasks SET status = ?1, updated_at = datetime('now') WHERE channel_id = ?2 AND task_number = ?3",
            params![new_status.as_str(), channel.id, task_number],
        )?;
        Ok(())
    }
}
