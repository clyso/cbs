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

import axios from 'axios';
import { useAuthStore } from '@/stores/auth';

const axiosInstance = axios.create({ withCredentials: true });

axiosInstance.interceptors.response.use(
  (response) => response,
  (error) => {
    if (error.response?.status === 401) {
      const authStore = useAuthStore();

      // Don't redirect while fetchUser is in flight — let the error propagate
      // so the access guard can handle it, otherwise we get a reload loop.
      if (!authStore.isLoading) {
        window.location.href = '/login';
      }
    }

    return Promise.reject(error);
  },
);

export default axiosInstance;
