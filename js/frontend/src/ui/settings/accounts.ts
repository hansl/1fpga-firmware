import { DEFAULT_USERNAME, User } from "@/services";
import * as osd from "1fpga:osd";
import { oneLine } from "common-tags";

async function addUser() {
  const username = await osd.prompt("Enter the new user's username:");
  if (!username) {
    return;
  }
  if (username === DEFAULT_USERNAME) {
    await osd.alert("Invalid username");
    return;
  }
  if ((await User.byUsername(username)) !== null) {
    await osd.alert("User already exists");
    return;
  }

  const user = await User.create(username, null, false);
  await osd.alert(oneLine`
    User '${username}' created successfully. 
    To set a password, login as the user and change the password.
  `);
  return user;
}

async function changePassword(user: User) {
  while (true) {
    const password = await osd.promptPassword(
      "Enter your new password:",
      "",
      4,
    );
    if (password === null) {
      return;
    }

    const password2 = await osd.promptPassword(
      "Verify your new password:",
      "",
      4,
    );
    if (password2 === null) {
      continue;
    }
    if (User.passwordToString(password) === User.passwordToString(password2)) {
      await user.setPassword(password);
      await osd.alert("Password changed successfully");
      return;
    }

    const choice = await osd.alert({
      message: "Passwords do not match. Please try again.",
      choices: ["Try Again", "Cancel"],
    });
    if (choice === 1) {
      return;
    }
  }
}

async function manageUser(user: User) {
  await osd.textMenu({
    title: "Manage User '" + user.username + "'",
    back: false,
    items: [
      {
        label: "Clear Password...",
        select: async () => {
          await user.clearPassword();
          await osd.alert("Password cleared successfully");
          return false;
        },
      },
      {
        label: "Delete User",
        select: async () => {
          const choice = await osd.alert({
            message: "Are you sure you want to delete this user?",
            choices: ["No", "Yes"],
          });
          if (choice === 0) {
            return;
          }
          await user.delete();
          await osd.alert("User deleted successfully");
          return false;
        },
      },
      {
        label: "Admin: ",
        marker: user.admin ? "Yes" : "No",
        select: async (item) => {
          await user.toggleAdmin();
          item.marker = user.admin ? "Yes" : "No";
          return false;
        },
      },
    ],
  });
}

/**
 * Show the accounts settings menu.
 * @returns Whether we need to refresh the main menu.
 */
export async function accountsSettingsMenu(): Promise<boolean> {
  const loggedInUser = User.loggedInUser(true);
  const users = (await User.list()).filter(
    (u) => u.id != loggedInUser.id && u.username != DEFAULT_USERNAME,
  );
  const items: osd.TextMenuItem<boolean>[] = [
    {
      label: "Clear Password",
      select: async () => {
        await loggedInUser.clearPassword();
      },
    },
    {
      label: "Change Password...",
      select: async () => await changePassword(loggedInUser),
    },
  ];

  if (loggedInUser.admin) {
    items.push(
      { label: "-" },
      {
        label: "Add User...",
        select: async () => {
          await addUser();
          reloadMainMenu = true;
        },
      },
    );
    if (users.length > 0) {
      items.push(
        { label: "-" },
        ...users.map((user) => ({
          label: user.username + (user.admin ? " (admin)" : ""),
          select: async () => {
            await manageUser(user);
            reloadMainMenu = true;
            return false;
          },
        })),
      );
    }
  }

  let done = false;
  let reloadMainMenu = false;
  while (!done) {
    done = await osd.textMenu({
      title: "Accounts",
      back: true,
      items,
    });
  }

  return reloadMainMenu;
}
