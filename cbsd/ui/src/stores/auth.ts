/*
 * Copyright © 2026 Clyso GmbH
 *
 *  Licensed under the GNU Affero General Public License, Version 3.0 (the "License");
 *  you may not use this file except in compliance with the License.
 *  You may obtain a copy of the License at
 *
 *  https://www.gnu.org/licenses/agpl-3.0.html
 *
 *  Unless required by applicable law or agreed to in writing, software
 *  distributed under the License is distributed on an "AS IS" BASIS,
 *  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 *  See the License for the specific language governing permissions and
 *  limitations under the License.
 */

import { defineStore } from 'pinia';
import { computed, reactive, toRef } from 'vue';
import type { User } from '@/utils/types/cbs';
import { CbsService } from '@/services/CbsService';
import { CookieHelper } from '@/utils/helpers/cookieHelper';

interface AuthState {
  user: User | null;
  token: string | undefined;
  isAuthenticated: boolean;
  hasUserError: boolean;
}

function getInitialState(): AuthState {
  const token = CookieHelper.getCookie();

  return {
    user: null as User | null,
    token: token,
    isAuthenticated: !!token,
    hasUserError: false,
  };
}

export const useAuthStore = defineStore('auth', () => {
  const state = reactive<AuthState>(getInitialState());

  const token = computed<string | undefined>(
    () => state.token || CookieHelper.getCookie(),
  );
  const isAuthenticated = computed<boolean>(() => !!token.value);

  function login() {
    CbsService.login();
  }

  function logout() {
    CookieHelper.removeCookie();
    state.token = undefined;
    state.user = null;
    state.isAuthenticated = false;
    window.location.href = '/login';
  }

  async function fetchUser() {
    try {
      state.user = await CbsService.getUser();
      state.hasUserError = false;
    } catch {
      state.hasUserError = true;
    }
  }

  return {
    token,
    login,
    logout,
    isAuthenticated,
    fetchUser,
    user: toRef(state, 'user'),
    hasUserError: toRef(state, 'hasUserError'),
  };
});
