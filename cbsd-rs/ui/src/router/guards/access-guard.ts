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

import { storeToRefs } from 'pinia';
import type { NavigationGuard } from 'vue-router';
import { useAuthStore } from '@/stores/auth';
import { RouteName } from '@/utils/types/router';

const accessGuard: NavigationGuard = (to, _from, next) => {
  const { isAuthenticated } = storeToRefs(useAuthStore());

  if (isAuthenticated.value && to.meta.isAuth?.()) {
    next({ name: RouteName.HOME });

    return;
  }

  if (!isAuthenticated.value && !to.meta.isPublic?.()) {
    next({
      name: RouteName.LOGIN,
      state: {
        redirect: to.fullPath,
      },
    });

    return;
  }

  if (to.meta.isAuthorized?.() === false) {
    next({ name: RouteName.HOME });

    return;
  }

  next();
};

export default accessGuard;
